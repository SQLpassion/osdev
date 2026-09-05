//! Interactive Phase-1 CLI loop shared by every NIC driver's foreground mode
//! (`ping`/`listen`/`ifconfig`/`info`/`set`/`arp`/`help`/`exit`).
//!
//! Command parsing and output formatting are split into pure, I/O-free
//! functions so they can be unit tested directly; `run_foreground_cli`
//! itself just wires those pieces to `println!`/device/stack calls and is
//! thin enough to be verified by manual QEMU smoke test instead (see
//! `docs/nic_driver_design.md`'s Phase-2-issue test-coverage notes).

extern crate alloc;
use alloc::string::String;

#[cfg(target_arch = "x86_64")]
use lib_kaos::{console, print, println, process};
use lib_net::{Ipv4Address, MacAddress};
#[cfg(target_arch = "x86_64")]
use lib_net::{NetworkEvent, NetworkStack, NicDevice};

/// Splits one input line into a command word and the (possibly empty)
/// remainder, trimming surrounding whitespace. Returns `None` for an empty
/// or whitespace-only line.
pub fn parse_command_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    match line.split_once(char::is_whitespace) {
        Some((cmd, rest)) => Some((cmd, rest.trim_start())),
        None => Some((line, "")),
    }
}

/// Error parsing a single required IPv4 argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv4ArgError<'a> {
    /// No argument was supplied at all.
    Missing,
    /// An argument was supplied but is not a valid dotted-quad IPv4 address.
    Invalid(&'a str),
}

/// Parses the first whitespace-separated token of `args` as an IPv4 address
/// (used by `ping <ip>`).
pub fn parse_single_ipv4_arg(args: &str) -> Result<Ipv4Address, Ipv4ArgError<'_>> {
    let token = args
        .split_whitespace()
        .next()
        .ok_or(Ipv4ArgError::Missing)?;
    Ipv4Address::parse_str(token).ok_or(Ipv4ArgError::Invalid(token))
}

/// Formats the ARP table for the `arp` command.
pub fn format_arp_table(entries: &[(Ipv4Address, MacAddress)]) -> String {
    if entries.is_empty() {
        return String::from("ARP table is empty.\n");
    }
    let mut out = String::from("Address                  HWaddress\n");
    for (ip, mac) in entries {
        out.push_str(&alloc::format!("{:<24} {}\n", ip, mac));
    }
    out
}

/// Formats the header line printed by a bare `ifconfig` (no arguments),
/// including the model name for drivers that have one (e.g. Intel NIC's
/// `82577LM`/`I219-V`).
pub fn ifconfig_header(interface_name: &str, model_name: Option<&str>) -> String {
    match model_name {
        Some(model) => alloc::format!("Interface {} ({}):", interface_name, model),
        None => alloc::format!("Interface {}:", interface_name),
    }
}

/// Reads the x86_64 timestamp counter for RTT measurement.
///
/// `#[cfg(target_arch = "x86_64")]`: the inline `rdtsc` asm only assembles
/// on x86_64. `cargo test -p lib_driver_runtime` additionally builds this
/// crate as a plain (non-`--cfg test`) library -- needed for doctests/
/// downstream validity -- so gating on `not(test)` alone would still pull
/// this in on a non-x86_64 development host (e.g. aarch64) and fail to
/// compile; the architecture check is the actual constraint.
#[cfg(target_arch = "x86_64")]
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

/// Executes the interactive `ping` command.
#[cfg(target_arch = "x86_64")]
fn execute_ping<D: NicDevice>(device: &mut D, stack: &mut NetworkStack, target_ip: Ipv4Address) {
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
        // Send initial ARP request for next-hop IP
        let mut arp_buf = [0u8; 64];
        let arp_len = stack
            .build_arp_request(next_hop_ip, &mut arp_buf)
            .unwrap_or(0);
        if arp_len > 0 {
            stack.tx_packets += 1;
            stack.tx_bytes += arp_len;
            let _ = device.transmit(&arp_buf[..arp_len]);
        }

        // Wait up to 20000ms for ARP resolution, retrying every 2000ms.
        // This is crucial because physical switches often block traffic for
        // several seconds (STP Listening/Learning states) after the link
        // physically comes up.
        let mut resolved_mac = None;
        let start_time = read_tsc();
        let mut last_arp_tx = start_time;
        let mut rx_buf = [0u8; 1792];

        while read_tsc().saturating_sub(start_time) < 20_000_000_000 {
            while let Some(len) = device.poll_next_packet(&mut rx_buf) {
                let _ = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                    let _ = device.transmit(tx_pkt);
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
                    let _ = device.transmit(&arp_buf[..arp_len]);
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
    let mut transmitted = 0;
    let mut received = 0;
    let mut rx_buf = [0u8; 1792];

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
        let _ = device.transmit(&ping_buf[..ping_len]);

        // Wait up to 2000ms for Echo Reply
        let mut got_reply = false;
        while read_tsc().saturating_sub(send_time) < 2_000_000_000 {
            let mut echo_event = None;
            while let Some(len) = device.poll_next_packet(&mut rx_buf) {
                let event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                    let _ = device.transmit(tx_pkt);
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
                // Approximate 2 GHz clock (2,000,000 cycles per ms)
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

        // 200ms delay between pings
        let pause_start = read_tsc();
        while read_tsc().saturating_sub(pause_start) < 400_000_000 {
            core::hint::spin_loop();
        }
    }

    // Step 4: Print summary statistics.
    println!("\n--- {} ping statistics ---", target_ip);
    let loss_pct = if transmitted > 0 {
        ((transmitted - received) * 100) / transmitted
    } else {
        0
    };
    println!(
        "{} packets transmitted, {} received, {}% packet loss",
        transmitted, received, loss_pct
    );
}

/// Executes background packet listening mode.
#[cfg(target_arch = "x86_64")]
fn execute_listen<D: NicDevice>(device: &mut D, stack: &mut NetworkStack, device_label: &str) {
    let mut rx_buf = [0u8; 1792];

    // Step 1: Drain any pending key events (e.g. Enter key from typing 'listen').
    while let Ok(key) = console::poll_key() {
        if key == console::Key::Unknown {
            break;
        }
    }

    // Step 2: Flush stale DMA RX packets accumulated while waiting at the CLI prompt.
    while device.poll_next_packet(&mut rx_buf).is_some() {}

    println!(
        "{} Listening for network packets (press any key to stop)...",
        device_label
    );

    loop {
        // Step 3: Poll keyboard to check if user wants to exit listening mode.
        if let Ok(key) = console::poll_key() {
            if key != console::Key::Unknown {
                println!("{} Stopped listening.", device_label);
                break;
            }
        }

        // Step 4: Process incoming packets from the RX ring/descriptor buffer.
        while let Some(len) = device.poll_next_packet(&mut rx_buf) {
            let event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                let _ = device.transmit(tx_pkt);
            });

            match event {
                NetworkEvent::ArpRequestAnswered {
                    sender_ip,
                    sender_mac,
                } => {
                    println!(
                        "[NET] Answered ARP Request: who-has {} tell {} ({})",
                        stack.config.ip, sender_ip, sender_mac
                    );
                }
                NetworkEvent::ArpReplyReceived {
                    sender_ip,
                    sender_mac,
                } => {
                    println!(
                        "[NET] Received ARP reply: {} is at {}",
                        sender_ip, sender_mac
                    );
                }
                NetworkEvent::IcmpEchoReply {
                    src_ip,
                    identifier,
                    sequence,
                    ..
                } => {
                    println!(
                        "[NET] Received ICMP Echo Reply from {} (id={:#x}, seq={})",
                        src_ip, identifier, sequence
                    );
                }
                NetworkEvent::IcmpEchoRequestAnswered { src_ip, sequence } => {
                    println!(
                        "[NET] Answered ICMP Echo Request from {} (seq={})",
                        src_ip, sequence
                    );
                }
                NetworkEvent::None => {}
            }
        }

        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
    }
}

/// Runs the interactive Phase-1 CLI loop (`ping`/`listen`/`ifconfig`/`info`/
/// `set`/`arp`/`help`/`exit`) against any `NicDevice` implementation.
///
/// - `device_label`: used in `println!` prefixes (e.g. `"[RTL8139]"` /
///   `"[Intel NIC]"`).
/// - `interface_name`: used in the bare-`ifconfig` header (e.g. `"rtl8139"`).
/// - `model_name`: `Some(name)` for drivers with a runtime-selected hardware
///   model (e.g. Intel's 82577LM/I219-V), printed as an extra `info`/
///   `ifconfig` line; `None` for drivers with a single fixed model.
/// - `prompt`: the REPL prompt string (e.g. `"[rtl8139]> "`).
///
/// Never returns except via `process::exit()` on `exit`/`quit` or a read error.
#[cfg(target_arch = "x86_64")]
pub fn run_foreground_cli<D: NicDevice>(
    mut device: D,
    mut stack: NetworkStack,
    device_label: &str,
    interface_name: &str,
    model_name: Option<&str>,
    prompt: &str,
) -> ! {
    println!("Type 'help' for available commands.\n");

    let mut line_buf = [0u8; 128];
    loop {
        print!("{}", prompt);
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
                println!(
                    "  info                          - Display network configuration & statistics"
                );
                println!("  ifconfig [ip] [gw] [mask]     - Display or configure IP, Gateway, and Subnet Mask");
                println!("  set ip <address>              - Set interface IPv4 address (e.g. set ip 192.168.1.200)");
                println!("  set gw <address>              - Set default gateway IPv4 address (e.g. set gw 192.168.1.1)");
                println!("  set mask <address>            - Set subnet mask (e.g. set mask 255.255.255.0)");
                println!("  set dns <address>             - Set DNS server address (e.g. set dns 192.168.1.1)");
                println!("  arp                           - Display dynamic ARP table entries");
                println!("  ping <ip>                     - Send ICMP Echo Requests to <ip> (e.g. ping 192.168.1.1)");
                println!("  listen                        - Listen for network packets and auto-respond to ping/arp");
                println!("  exit, quit                    - Exit driver and return to KAOS shell");
            }
            "info" => {
                println!("--- Network Interface Configuration ---");
                if let Some(model) = model_name {
                    println!("  Hardware Model: {}", model);
                }
                println!("  Hardware MAC : {}", stack.config.mac);
                println!("  IPv4 Address : {}", stack.config.ip);
                println!("  Subnet Mask  : {}", stack.config.subnet_mask);
                println!("  Gateway IP   : {}", stack.config.gateway);
                println!("  DNS Server   : {}", stack.config.dns);
                println!("--- Packet Statistics ---");
                println!(
                    "  RX Packets   : {} ({} bytes)",
                    stack.rx_packets, stack.rx_bytes
                );
                println!(
                    "  TX Packets   : {} ({} bytes)",
                    stack.tx_packets, stack.tx_bytes
                );
            }
            "ifconfig" => {
                let mut parts = args.split_whitespace();
                if let Some(ip_str) = parts.next() {
                    let Some(new_ip) = Ipv4Address::parse_str(ip_str) else {
                        println!("Invalid IPv4 address: '{}'", ip_str);
                        continue;
                    };
                    stack.config.ip = new_ip;

                    if let Some(gw_str) = parts.next() {
                        if let Some(new_gw) = Ipv4Address::parse_str(gw_str) {
                            stack.config.gateway = new_gw;
                        }
                    }

                    if let Some(mask_str) = parts.next() {
                        if let Some(new_mask) = Ipv4Address::parse_str(mask_str) {
                            stack.config.subnet_mask = new_mask;
                        }
                    }

                    println!(
                        "Configured network: IP {}, Gateway {}, Mask {}",
                        stack.config.ip, stack.config.gateway, stack.config.subnet_mask
                    );
                } else {
                    println!("{}", ifconfig_header(interface_name, model_name));
                    println!("  MAC address  : {}", stack.config.mac);
                    println!("  inet addr    : {}", stack.config.ip);
                    println!("  gateway      : {}", stack.config.gateway);
                    println!("  netmask      : {}", stack.config.subnet_mask);
                    println!("  nameserver   : {}", stack.config.dns);
                }
            }
            "set" => {
                let mut parts = args.split_whitespace();
                let Some(subcmd) = parts.next() else {
                    println!("Usage: set <ip|gw|mask|dns> <address>");
                    continue;
                };
                let Some(val_str) = parts.next() else {
                    println!("Usage: set {} <address>", subcmd);
                    continue;
                };
                let Some(parsed_ip) = Ipv4Address::parse_str(val_str) else {
                    println!("Invalid IPv4 address: '{}'", val_str);
                    continue;
                };

                match subcmd {
                    "ip" | "addr" => {
                        stack.config.ip = parsed_ip;
                        println!("Interface IP address set to {}", stack.config.ip);
                    }
                    "gw" | "gateway" => {
                        stack.config.gateway = parsed_ip;
                        println!("Default gateway set to {}", stack.config.gateway);
                    }
                    "mask" | "netmask" => {
                        stack.config.subnet_mask = parsed_ip;
                        println!("Subnet mask set to {}", stack.config.subnet_mask);
                    }
                    "dns" => {
                        stack.config.dns = parsed_ip;
                        println!("DNS server set to {}", stack.config.dns);
                    }
                    _ => {
                        println!(
                            "Unknown parameter: '{}'. Use 'ip', 'gw', 'mask', or 'dns'.",
                            subcmd
                        );
                    }
                }
            }
            "arp" => {
                print!("{}", format_arp_table(stack.arp_table.entries()));
            }
            "ping" => match parse_single_ipv4_arg(args) {
                Ok(target_ip) => execute_ping(&mut device, &mut stack, target_ip),
                Err(Ipv4ArgError::Missing) => {
                    println!("Usage: ping <ipv4_address> (e.g. ping 192.168.1.1)");
                }
                Err(Ipv4ArgError::Invalid(s)) => {
                    println!("Invalid IPv4 address: '{}'", s);
                }
            },
            "listen" => {
                execute_listen(&mut device, &mut stack, device_label);
            }
            "exit" | "quit" => {
                println!("{} Shutting down device...", device_label);
                device.shutdown();
                break;
            }
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

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/repl.rs"]
mod tests;
