//! Pure, I/O-free logic for `net-tools`: command parsing, output formatting,
//! ping statistics, and driver-name probe order -- split out from the real
//! syscalls/console I/O around them so it can be unit tested directly, per
//! this project's convention (see `user_programs/rtl8139/src/tests/rtl8139.rs`).

extern crate alloc;
use alloc::string::String;

use lib_driver::{UserDriverStatus, MAX_ARP_ENTRIES};
use lib_net::Ipv4Address;

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

/// Error parsing `ping <ip>`'s required IPv4 argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingArgError<'a> {
    /// No argument was supplied at all.
    Missing,
    /// An argument was supplied but is not a valid dotted-quad IPv4 address.
    Invalid(&'a str),
}

/// Parses the first whitespace-separated token of `args` as an IPv4 address.
pub fn parse_ping_target(args: &str) -> Result<Ipv4Address, PingArgError<'_>> {
    let token = args
        .split_whitespace()
        .next()
        .ok_or(PingArgError::Missing)?;
    Ipv4Address::parse_str(token).ok_or(PingArgError::Invalid(token))
}

/// Selects the first name in `names` for which `is_registered` returns
/// `true`, trying them in order (Task 2's startup probing algorithm).
/// Returns `None` if none are registered.
pub fn probe_driver_name<'a>(
    names: &[&'a str],
    is_registered: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    names.iter().copied().find(|&name| is_registered(name))
}

/// Formats one queried `UserDriverStatus`'s ARP table for the `arp` command,
/// matching the driver CLI's `Address / HWaddress` column convention
/// (`lib_driver_runtime::repl::format_arp_table`).
pub fn format_arp_table(status: &UserDriverStatus) -> String {
    let count = (status.arp_entry_count as usize).min(MAX_ARP_ENTRIES);
    if count == 0 {
        return String::from("ARP table is empty.\n");
    }
    let mut out = String::from("Address                  HWaddress\n");
    for entry in &status.arp_entries[..count] {
        out.push_str(&alloc::format!(
            "{:<24} {}\n",
            format_ipv4(entry.ip),
            format_mac(entry.mac)
        ));
    }
    out
}

/// Formats one queried `UserDriverStatus` for the `ifconfig` command.
pub fn format_ifconfig(status: &UserDriverStatus) -> String {
    alloc::format!(
        "MAC address  : {}\ninet addr    : {}\ngateway      : {}\nnetmask      : {}\nnameserver   : {}\nlink         : {}\nRX Packets   : {} ({} bytes)\nTX Packets   : {} ({} bytes)\n",
        format_mac(status.mac),
        format_ipv4(status.ip),
        format_ipv4(status.gateway),
        format_ipv4(status.subnet_mask),
        format_ipv4(status.dns),
        if status.link_up != 0 { "up" } else { "down" },
        status.rx_packets,
        status.rx_bytes,
        status.tx_packets,
        status.tx_bytes,
    )
}

fn format_ipv4(octets: [u8; 4]) -> Ipv4Address {
    Ipv4Address::new(octets[0], octets[1], octets[2], octets[3])
}

fn format_mac(octets: [u8; 6]) -> lib_net::MacAddress {
    lib_net::MacAddress::new(octets)
}

/// Computes the packet-loss percentage for a completed ping run, matching
/// the driver CLI's original `((transmitted - received) * 100) / transmitted`
/// formula (0 if nothing was transmitted).
pub fn ping_loss_percent(transmitted: u32, received: u32) -> u32 {
    ((transmitted - received) * 100)
        .checked_div(transmitted)
        .unwrap_or(0)
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/util.rs"]
mod tests;
