//! Command parsing and output formatting shared by KAOS's interactive NIC
//! tools (originally the Phase-1 driver-embedded CLI, now reused by the
//! planned `net-tools.bin`, Phase 2 Step 8), plus the driver's permanent
//! background event loop (Phase 2 Step 6).
//!
//! Everything below is pure, I/O-free logic so it can be unit tested
//! directly; `run_background_driver` (real syscalls, real hardware I/O) is
//! the one exception. It is instead covered indirectly by
//! `kernel/tests/driver_background_loop_test.rs`, which replicates its exact
//! steps against the real kernel syscalls -- see that file's header comment
//! for why the function itself can't be called from that harness.

extern crate alloc;
use alloc::string::String;

use lib_net::{Ipv4Address, MacAddress};

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

/// Translates a `NetworkStack`'s configuration, counters, and ARP table into
/// a `UserDriverStatus` snapshot for `DrvPublishStatus`.
///
/// `link_up` is always reported as up: the current `NicDevice` trait has no
/// link-state query, so this is the best signal available -- a driver that
/// reaches its background loop at all has already successfully initialized
/// its hardware.
///
/// If the live ARP table holds more than `MAX_ARP_ENTRIES` resolved hosts,
/// the excess is silently truncated here (not reported as an error) --
/// `DrvPublishStatus` itself rejects an over-count outright (Step 3), so
/// truncation has to happen on this side of that boundary.
pub fn build_status(stack: &lib_net::NetworkStack) -> lib_driver::UserDriverStatus {
    let cfg = &stack.config;
    let entries = stack.arp_table.entries();
    let arp_entry_count = entries.len().min(lib_driver::MAX_ARP_ENTRIES);

    let mut arp_entries = [lib_driver::UserArpEntry {
        ip: [0; 4],
        mac: [0; 6],
        _padding: [0; 2],
    }; lib_driver::MAX_ARP_ENTRIES];
    for (slot, (ip, mac)) in arp_entries
        .iter_mut()
        .zip(entries.iter())
        .take(arp_entry_count)
    {
        slot.ip = ip.0;
        slot.mac = mac.0;
    }

    lib_driver::UserDriverStatus {
        mac: cfg.mac.0,
        _padding0: [0; 2],
        ip: cfg.ip.0,
        subnet_mask: cfg.subnet_mask.0,
        gateway: cfg.gateway.0,
        dns: cfg.dns.0,
        rx_packets: stack.rx_packets as u64,
        rx_bytes: stack.rx_bytes as u64,
        tx_packets: stack.tx_packets as u64,
        tx_bytes: stack.tx_bytes as u64,
        link_up: 1,
        _padding1: [0; 3],
        arp_entry_count: arp_entry_count as u32,
        arp_entries,
    }
}

/// Runs a driver's permanent background event loop.
///
/// Registers under `driver_name` (e.g. `"nic:rtl8139"`), then forever:
/// 1. Drains queued app-to-driver packets (`NetRecv` on the driver's own
///    tid -- the role-based direction rule from Phase 2 Step 2 routes these
///    to the TX ring) and transmits them.
/// 2. Polls the hardware for received frames, feeds them through the
///    `NetworkStack` (ARP/ICMP auto-replies handled exactly as the old
///    foreground CLI did), and forwards each frame to waiting apps
///    (`NetSend` on the driver's own tid, landing in its RX ring).
/// 3. Publishes an updated `UserDriverStatus` (`DrvPublishStatus`) every
///    iteration so `DrvQuery` consumers (Step 8's `net-tools.bin`) see
///    current data.
/// 4. Yields (`process::yield_now()`) so this cooperative loop never starves
///    other tasks.
///
/// Never returns under normal operation.
#[cfg(target_arch = "x86_64")]
pub fn run_background_driver<D: lib_net::NicDevice>(
    mut device: D,
    mut stack: lib_net::NetworkStack,
    driver_name: &str,
) -> ! {
    // Step 0: register, then resolve our own packed task id the same way
    // any other caller would -- there is no separate "get my own tid"
    // syscall, and DrvRegister itself does not return one.
    if let Err(e) = lib_driver::drv::drv_register(driver_name.as_bytes()) {
        lib_kaos::serial_println!("[driver] DrvRegister failed: {:?}", e);
        lib_kaos::process::exit();
    }
    let own_id = match lib_driver::drv::drv_lookup(driver_name.as_bytes()) {
        Ok(id) => id,
        Err(e) => {
            lib_kaos::serial_println!("[driver] DrvLookup of own name failed: {:?}", e);
            lib_kaos::process::exit();
        }
    };

    let mut tx_buf = [0u8; lib_driver::drv::MAX_PACKET_LEN];
    let mut rx_buf = [0u8; lib_driver::drv::MAX_PACKET_LEN];
    loop {
        // Step 1: drain app -> driver TX requests (non-blocking: timeout_ms
        // == 0 polls once and returns Timeout immediately once the ring is
        // empty, per NetRecv's documented deviation from IrqWait's "0 =
        // wait forever" convention).
        while let Ok(len) = lib_driver::drv::net_recv(own_id, &mut tx_buf, 0) {
            let _ = device.transmit(&tx_buf[..len]);
        }

        // Step 2: poll hardware RX, run the protocol stack, forward every
        // frame to waiting apps regardless of what the stack did with it.
        while let Some(len) = device.poll_next_packet(&mut rx_buf) {
            let _event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                let _ = device.transmit(tx_pkt);
            });
            let _ = lib_driver::drv::net_send(own_id, &rx_buf[..len]);
        }

        // Step 3: publish current status for DrvQuery consumers.
        let status = build_status(&stack);
        let _ = lib_driver::drv::publish_status(&status);

        // Step 4: cooperative yield -- never busy-spin without one.
        lib_kaos::process::yield_now();
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/repl.rs"]
mod tests;
