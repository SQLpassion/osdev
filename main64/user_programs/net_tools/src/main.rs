//! `net-tools`: a standalone Ring-3 network utility that talks to whichever
//! NIC driver is currently `load`ed (Phase 2 Step 8 of
//! `docs/nic_driver_design.md`), instead of any network functionality being
//! baked into the driver binaries themselves.
//!
//! There is no argv support anywhere in this codebase yet
//! (`lib_kaos::process::exec`/`exec_from_vfs` take only a filename), so this
//! is an interactive REPL, exactly like the driver binaries' old
//! foreground CLI and `kbasic.bin`.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(dead_code)]

extern crate alloc;

mod util;

#[cfg(not(test))]
use lib_kaos::{console, print, println, process};
#[cfg(not(test))]
use lib_net::{Ipv4Address, MacAddress, NetworkEvent, NetworkStack};
#[cfg(not(test))]
use lib_net_client::NicClient;
#[cfg(not(test))]
use util::{
    format_arp_table, format_ifconfig, parse_command_line, parse_ping_target, ping_loss_percent,
    probe_driver_name, PingArgError,
};

/// Driver names this tool knows how to find, in probe order. Must match the
/// names `lib_driver_runtime::run_background_driver` registers under
/// (Phase 2 Step 6): `"nic:rtl8139"` / `"nic:intel_nic"`.
#[cfg(not(test))]
const KNOWN_DRIVER_NAMES: &[&str] = &["nic:rtl8139", "nic:intel_nic"];

#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("==================================================");
    println!("  KAOS net-tools (Ring 3)");
    println!("==================================================");

    // Step 1: probe each known driver name in order, stopping at the first
    // one currently registered.
    let Some(driver_name) =
        probe_driver_name(KNOWN_DRIVER_NAMES, |name| NicClient::open(name).is_ok())
    else {
        println!("No NIC driver loaded. Run 'load <name>.drv' from the shell first.");
        process::exit();
    };

    // Step 2: open the client for real. NicClient::open is a cheap,
    // side-effect-free DrvLookup, so re-resolving the name probe_driver_name
    // already verified is registered is not a meaningful extra cost.
    let client = match NicClient::open(driver_name) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "[net-tools] Failed to open driver '{}': {:?}",
                driver_name, e
            );
            process::exit();
        }
    };
    println!("[net-tools] Connected to driver '{}'.", driver_name);

    // Step 3: seed a local NetworkStack from the driver's published status,
    // so the ported ping algorithm's routing/subnet-check logic works
    // correctly without net-tools ever touching hardware.
    let status = match client.query_status() {
        Ok(s) => s,
        Err(e) => {
            println!("[net-tools] DrvQuery failed: {:?}", e);
            process::exit();
        }
    };
    let mac = MacAddress::new(status.mac);
    let mut stack = NetworkStack::new(mac);
    stack.config.ip = Ipv4Address::new(status.ip[0], status.ip[1], status.ip[2], status.ip[3]);
    stack.config.subnet_mask = Ipv4Address::new(
        status.subnet_mask[0],
        status.subnet_mask[1],
        status.subnet_mask[2],
        status.subnet_mask[3],
    );
    stack.config.gateway = Ipv4Address::new(
        status.gateway[0],
        status.gateway[1],
        status.gateway[2],
        status.gateway[3],
    );
    stack.config.dns = Ipv4Address::new(status.dns[0], status.dns[1], status.dns[2], status.dns[3]);

    println!("Type 'help' for available commands.\n");

    // Step 4: REPL loop.
    let mut line_buf = [0u8; 128];
    loop {
        print!("net-tools> ");
        let len = match console::readline(&mut line_buf) {
            Ok(l) => l,
            Err(_) => break,
        };
        let Ok(line) = core::str::from_utf8(&line_buf[..len]) else {
            continue;
        };
        let Some((cmd, args)) = parse_command_line(line) else {
            continue;
        };

        match cmd {
            "help" => {
                println!("Available commands:");
                println!("  ping <ip>  - send 4 ICMP Echo Requests to <ip>");
                println!("  arp        - display the driver's resolved ARP table");
                println!("  ifconfig   - display the driver's current network configuration");
                println!("  exit, quit - exit net-tools and return to the KAOS shell");
            }
            "ping" => match parse_ping_target(args) {
                Ok(target_ip) => execute_ping(&client, &mut stack, target_ip),
                Err(PingArgError::Missing) => {
                    println!("Usage: ping <ipv4_address> (e.g. ping 192.168.1.1)");
                }
                Err(PingArgError::Invalid(s)) => {
                    println!("Invalid IPv4 address: '{}'", s);
                }
            },
            "arp" => match client.query_status() {
                Ok(s) => print!("{}", format_arp_table(&s)),
                Err(e) => println!("[net-tools] DrvQuery failed: {:?}", e),
            },
            "ifconfig" => match client.query_status() {
                Ok(s) => print!("{}", format_ifconfig(&s)),
                Err(e) => println!("[net-tools] DrvQuery failed: {:?}", e),
            },
            "exit" | "quit" => break,
            _ => {
                println!(
                    "Unknown command: '{}'. Type 'help' for available commands.",
                    cmd
                );
            }
        }
    }

    process::exit()
}

/// Reads the x86_64 timestamp counter for RTT measurement.
#[cfg(not(test))]
#[inline]
fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY:
    // - RDTSC instruction is accessible in Ring 3 and has no memory side effects.
    unsafe {
        core::arch::asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack));
    }
    ((high as u64) << 32) | (low as u64)
}

/// Executes the `ping` command against `client` instead of a `NicDevice`.
///
/// This is a direct port of the driver CLI's original `execute_ping`
/// (formerly `user_programs/rtl8139/src/main.rs`, later
/// `lib_driver_runtime::repl::execute_ping` before Phase 2 Step 6 removed
/// the driver-embedded foreground CLI entirely): the ARP-resolve-then-ICMP-
/// echo algorithm itself is unchanged and still comes from `lib_net`'s
/// hardware-agnostic `NetworkStack` APIs (`build_arp_request`,
/// `handle_rx_packet`, `build_ping`). Only the two hardware-coupled calls
/// change: `device.transmit(&buf)` becomes `client.send(&buf)`, and
/// `device.poll_next_packet(&mut rx_buf)` (`Option<usize>`, "did a packet
/// arrive") becomes a `while let Ok(len) = client.recv(&mut rx_buf, 0)`
/// non-blocking poll -- both loop shapes drain exactly as many queued
/// frames as are available and then fall through, including on
/// `Err(SysError::Timeout)` (nothing queued) or any other transient error,
/// which must not crash this REPL.
#[cfg(not(test))]
fn execute_ping(client: &NicClient, stack: &mut NetworkStack, target_ip: Ipv4Address) {
    println!("PING {} 56(84) bytes of data.", target_ip);

    // Step 1: Determine routing next-hop (direct subnet or default gateway).
    let is_local = stack
        .config
        .ip
        .is_same_subnet(target_ip, stack.config.subnet_mask);
    let next_hop_ip = if is_local {
        target_ip
    } else {
        if stack.config.gateway.is_zero() {
            println!(
                "From {}: Destination Host Unreachable (no default gateway configured for remote subnet)",
                stack.config.ip
            );
            return;
        }
        stack.config.gateway
    };

    // Step 2: Resolve next-hop MAC address via ARP if not cached.
    let dest_mac = if let Some(mac) = stack.arp_table.lookup(next_hop_ip) {
        mac
    } else {
        let mut arp_buf = [0u8; 64];
        let arp_len = stack
            .build_arp_request(next_hop_ip, &mut arp_buf)
            .unwrap_or(0);
        if arp_len > 0 {
            stack.tx_packets += 1;
            stack.tx_bytes += arp_len;
            let _ = client.send(&arp_buf[..arp_len]);
        }

        // Wait up to 20000ms for ARP resolution, retrying every 2000ms.
        let mut resolved_mac = None;
        let start_time = read_tsc();
        let mut last_arp_tx = start_time;
        let mut rx_buf = [0u8; lib_driver::drv::MAX_PACKET_LEN];

        while read_tsc().saturating_sub(start_time) < 20_000_000_000 {
            while let Ok(len) = client.recv(&mut rx_buf, 0) {
                let _ = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                    let _ = client.send(tx_pkt);
                });
            }

            if let Some(mac) = stack.arp_table.lookup(next_hop_ip) {
                resolved_mac = Some(mac);
                break;
            }

            let current_time = read_tsc();
            if current_time.saturating_sub(last_arp_tx) >= 2_000_000_000 {
                if arp_len > 0 {
                    stack.tx_packets += 1;
                    stack.tx_bytes += arp_len;
                    let _ = client.send(&arp_buf[..arp_len]);
                }
                last_arp_tx = current_time;
            }

            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        }

        match resolved_mac {
            Some(m) => m,
            None => {
                println!(
                    "From {}: Destination Host Unreachable (ARP timeout for next-hop {})",
                    stack.config.ip, next_hop_ip
                );
                return;
            }
        }
    };

    // Step 3: Send 4 ICMP Echo Requests and measure RTT.
    let mut transmitted: u32 = 0;
    let mut received: u32 = 0;
    let mut rx_buf = [0u8; lib_driver::drv::MAX_PACKET_LEN];

    for seq in 1..=4 {
        transmitted += 1;
        let payload = b"KAOS Ping Payload 1234567890";
        let mut ping_buf = [0u8; 128];
        let Some(ping_len) = stack.build_ping(
            target_ip,
            dest_mac,
            0x1337,
            seq as u16,
            payload,
            &mut ping_buf,
        ) else {
            println!("Failed to format ping packet");
            continue;
        };

        stack.tx_packets += 1;
        stack.tx_bytes += ping_len;
        let send_time = read_tsc();
        let _ = client.send(&ping_buf[..ping_len]);

        // Wait up to 2000ms for Echo Reply.
        let mut got_reply = false;
        while read_tsc().saturating_sub(send_time) < 2_000_000_000 {
            let mut echo_event = None;
            while let Ok(len) = client.recv(&mut rx_buf, 0) {
                let event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                    let _ = client.send(tx_pkt);
                });
                if let NetworkEvent::IcmpEchoReply {
                    src_ip,
                    identifier,
                    sequence,
                    ttl,
                    data_len,
                } = event
                {
                    if src_ip == target_ip && identifier == 0x1337 && sequence == seq as u16 {
                        echo_event = Some((ttl, data_len));
                    }
                }
            }

            if let Some((ttl, _data_len)) = echo_event {
                let end_time = read_tsc();
                let cycles = end_time.saturating_sub(send_time);
                let ms = cycles / 2_000_000;
                let tenths = (cycles % 2_000_000) / 200_000;

                println!(
                    "64 bytes from {}: icmp_seq={} ttl={} time={}.{} ms",
                    target_ip, seq, ttl, ms, tenths
                );
                received += 1;
                got_reply = true;
                break;
            }

            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        }

        if !got_reply {
            println!("Request timeout for icmp_seq={}", seq);
        }

        let pause_start = read_tsc();
        while read_tsc().saturating_sub(pause_start) < 400_000_000 {
            core::hint::spin_loop();
        }
    }

    // Step 4: Print summary statistics.
    println!("\n--- {} ping statistics ---", target_ip);
    println!(
        "{} packets transmitted, {} received, {}% packet loss",
        transmitted,
        received,
        ping_loss_percent(transmitted, received)
    );
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
