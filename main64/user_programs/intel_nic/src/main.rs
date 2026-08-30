//! Intel Gigabit Ethernet (82577LM / I219-V) PCI user-space device driver & interactive CLI.
//!
//! Runs in Ring 3 with hardware isolation using `lib_driver` (`Mmio`, `Irq`, `Dma`) and `lib_net`.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(dead_code)]

extern crate alloc;

pub mod intel_nic;

#[cfg(not(test))]
use intel_nic::{IntelNicDevice, NicModel};
#[cfg(not(test))]
use lib_driver::mmio::Mmio;
#[cfg(not(test))]
use lib_kaos::{console, pci, print, println, process};
#[cfg(not(test))]
use lib_net::{Ipv4Address, NetworkEvent, NetworkStack, NicDevice};

#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("==================================================");
    println!("  KAOS Intel Gigabit Ethernet Driver (Ring 3)");
    println!("  Supports 82577LM (8086:10EA) & I219-V (8086:15B8)");
    println!("==================================================");

    // Step 1: Discover Intel NIC device via PCI subsystem.
    let dev_count = pci::get_pci_device_count().unwrap_or(0);
    let mut found = None;

    for i in 0..dev_count {
        let mut dev = pci::UserPciDevice {
            bus: 0,
            device: 0,
            function: 0,
            class_code: 0,
            subclass: 0,
            prog_if: 0,
            revision_id: 0,
            header_type: 0,
            vendor_id: 0,
            device_id: 0,
            interrupt_line: 0,
            interrupt_pin: 0,
            _padding: [0; 2],
            bars: [pci::UserPciBar {
                bar_type: 0,
                flags: 0,
                address: 0,
                size: 0,
                raw_value: 0,
                _padding: 0,
            }; 6],
        };

        if pci::get_pci_device(i, &mut dev).is_ok() && dev.vendor_id == 0x8086 {
            if dev.device_id == 0x10EA {
                found = Some((dev, NicModel::E1000e));
                break;
            } else if dev.device_id == 0x15B8 {
                found = Some((dev, NicModel::I219V));
                break;
            }
        }
    }

    let Some((dev, model)) = found else {
        println!("[Intel NIC] Error: No Intel 82577LM (8086:10EA) or I219-V (8086:15B8) PCI device found.");
        process::exit();
    };

    println!(
        "[Intel NIC] Found {}: PCI Bus {:02x}:{:02x}.{:x}, IRQ Line {}",
        model.name(),
        dev.bus,
        dev.device,
        dev.function,
        dev.interrupt_line
    );

    // Step 2: Locate MMIO BAR (BAR 0 is primary MMIO on Intel NICs).
    let mut mmio_bar = None;
    for bar in &dev.bars {
        if (bar.bar_type == 2 || bar.bar_type == 3) && bar.address != 0 {
            mmio_bar = Some(*bar);
            break;
        }
    }
    let (bar_phys, bar_size) = match mmio_bar {
        Some(b) => (
            b.address,
            if b.size != 0 {
                b.size as usize
            } else {
                128 * 1024
            },
        ),
        None => {
            if dev.bars[0].address != 0 {
                (
                    dev.bars[0].address,
                    if dev.bars[0].size != 0 {
                        dev.bars[0].size as usize
                    } else {
                        128 * 1024
                    },
                )
            } else {
                println!("[Intel NIC] Error: BAR 0 MMIO address is 0.");
                process::exit();
            }
        }
    };

    // Step 3: Map physical MMIO registers.
    let mmio = match Mmio::map(bar_phys, bar_size) {
        Ok(m) => m,
        Err(e) => {
            println!("[Intel NIC] Failed to map MMIO registers: {:?}", e);
            process::exit();
        }
    };

    // Step 4: Initialize Intel controller and DMA descriptor rings.
    let mut device = match IntelNicDevice::init(model, mmio, dev.interrupt_line) {
        Ok(d) => d,
        Err(e) => {
            println!("[Intel NIC] Device initialization failed: {:?}", e);
            process::exit();
        }
    };

    let mac = device.mac();
    println!("[Intel NIC] Hardware MAC Address: {}", mac);

    // Step 5: Initialize protocol network stack.
    let mut stack = NetworkStack::new(mac);
    println!(
        "[Intel NIC] Network initialized: IP {}, Gateway {}",
        stack.config.ip, stack.config.gateway
    );
    println!("Type 'help' for available commands.\n");

    // Step 6: Interactive driver CLI console loop.
    let mut line_buf = [0u8; 128];
    loop {
        print!("[intel_nic]> ");
        let len = match console::readline(&mut line_buf) {
            Ok(l) => l,
            Err(_) => break,
        };

        let Ok(line) = core::str::from_utf8(&line_buf[..len]) else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else {
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
                println!("  Hardware Model: {}", device.model().name());
                println!("  Hardware MAC  : {}", stack.config.mac);
                println!("  IPv4 Address  : {}", stack.config.ip);
                println!("  Subnet Mask   : {}", stack.config.subnet_mask);
                println!("  Gateway IP    : {}", stack.config.gateway);
                println!("  DNS Server    : {}", stack.config.dns);
                println!("--- Packet Statistics ---");
                println!(
                    "  RX Packets    : {} ({} bytes)",
                    stack.rx_packets, stack.rx_bytes
                );
                println!(
                    "  TX Packets    : {} ({} bytes)",
                    stack.tx_packets, stack.tx_bytes
                );
            }
            "ifconfig" => {
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
                    println!("Interface intel_nic ({}):", device.model().name());
                    println!("  MAC address  : {}", stack.config.mac);
                    println!("  inet addr    : {}", stack.config.ip);
                    println!("  gateway      : {}", stack.config.gateway);
                    println!("  netmask      : {}", stack.config.subnet_mask);
                    println!("  nameserver   : {}", stack.config.dns);
                }
            }
            "set" => {
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
                let entries = stack.arp_table.entries();
                if entries.is_empty() {
                    println!("ARP table is empty.");
                } else {
                    println!("Address                  HWaddress");
                    for (ip, mac) in entries {
                        println!("{:<24} {}", ip, mac);
                    }
                }
            }
            "ping" => {
                let Some(target_str) = parts.next() else {
                    println!("Usage: ping <ipv4_address> (e.g. ping 192.168.1.1)");
                    continue;
                };

                let Some(target_ip) = Ipv4Address::parse_str(target_str) else {
                    println!("Invalid IPv4 address: '{}'", target_str);
                    continue;
                };

                execute_ping(&mut device, &mut stack, target_ip);
            }
            "listen" => {
                execute_listen(&mut device, &mut stack);
            }
            "exit" | "quit" => {
                println!("[Intel NIC] Shutting down device...");
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

/// Executes the interactive `ping` command.
#[cfg(not(test))]
fn execute_ping(device: &mut impl NicDevice, stack: &mut NetworkStack, target_ip: Ipv4Address) {
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
        // Send ARP request for next-hop IP
        let mut arp_buf = [0u8; 64];
        if let Some(arp_len) = stack.build_arp_request(next_hop_ip, &mut arp_buf) {
            stack.tx_packets += 1;
            stack.tx_bytes += arp_len;
            let _ = device.transmit(&arp_buf[..arp_len]);
        }

        // Wait up to 1000ms for ARP resolution
        let mut resolved_mac = None;
        let start_time = read_tsc();
        let mut rx_buf = [0u8; 1792];
        while read_tsc().saturating_sub(start_time) < 2_000_000_000 {
            while let Some(len) = device.poll_next_packet(&mut rx_buf) {
                let _ = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                    let _ = device.transmit(tx_pkt);
                });
            }

            if let Some(mac) = stack.arp_table.lookup(next_hop_ip) {
                resolved_mac = Some(mac);
                break;
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

        // Wait up to 1000ms for Echo Reply
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
#[cfg(not(test))]
fn execute_listen(device: &mut impl NicDevice, stack: &mut NetworkStack) {
    let mut rx_buf = [0u8; 1792];

    // Step 1: Drain any pending key events (e.g. Enter key from typing 'listen').
    while let Ok(key) = console::poll_key() {
        if key == console::Key::Unknown {
            break;
        }
    }

    // Step 2: Flush stale DMA RX packets accumulated while waiting at the CLI prompt.
    while device.poll_next_packet(&mut rx_buf).is_some() {}

    println!("[Intel NIC] Listening for network packets (press any key to stop)...");

    loop {
        // Step 3: Poll keyboard to check if user wants to exit listening mode.
        if let Ok(key) = console::poll_key() {
            if key != console::Key::Unknown {
                println!("[Intel NIC] Stopped listening.");
                break;
            }
        }

        // Step 4: Process incoming packets from the RX descriptor ring.
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
