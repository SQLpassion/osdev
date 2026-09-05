use super::{
    format_arp_table, format_ifconfig, parse_command_line, parse_ping_target, ping_loss_percent,
    probe_driver_name, PingArgError,
};
use lib_driver::{UserArpEntry, UserDriverStatus, MAX_ARP_ENTRIES};
use lib_net::Ipv4Address;

fn sample_status(arp_entry_count: u32, link_up: u8) -> UserDriverStatus {
    let mut arp_entries = [UserArpEntry {
        ip: [0; 4],
        mac: [0; 6],
        _padding: [0; 2],
    }; MAX_ARP_ENTRIES];
    for (i, entry) in arp_entries
        .iter_mut()
        .enumerate()
        .take(arp_entry_count as usize)
    {
        entry.ip = [10, 0, 2, i as u8];
        entry.mac = [0x52, 0x54, 0x00, 0x12, 0x34, i as u8];
    }
    UserDriverStatus {
        mac: [0x52, 0x54, 0x00, 0x00, 0x00, 0x01],
        _padding0: [0; 2],
        ip: [10, 0, 2, 15],
        subnet_mask: [255, 255, 255, 0],
        gateway: [10, 0, 2, 2],
        dns: [8, 8, 8, 8],
        rx_packets: 42,
        rx_bytes: 4242,
        tx_packets: 24,
        tx_bytes: 2424,
        link_up,
        _padding1: [0; 3],
        arp_entry_count,
        arp_entries,
    }
}

// ---------------------------------------------------------------------------
// parse_command_line
// ---------------------------------------------------------------------------

#[test]
fn test_parse_command_line_empty_and_command_only() {
    assert_eq!(parse_command_line(""), None);
    assert_eq!(parse_command_line("   "), None);
    assert_eq!(parse_command_line("help"), Some(("help", "")));
}

#[test]
fn test_parse_command_line_with_args() {
    assert_eq!(
        parse_command_line("ping 10.0.2.2"),
        Some(("ping", "10.0.2.2"))
    );
}

// ---------------------------------------------------------------------------
// parse_ping_target
// ---------------------------------------------------------------------------

#[test]
fn test_parse_ping_target_well_formed() {
    assert_eq!(
        parse_ping_target("10.0.2.2"),
        Ok(Ipv4Address::new(10, 0, 2, 2))
    );
}

#[test]
fn test_parse_ping_target_missing() {
    assert_eq!(parse_ping_target(""), Err(PingArgError::Missing));
    assert_eq!(parse_ping_target("   "), Err(PingArgError::Missing));
}

#[test]
fn test_parse_ping_target_malformed_variants() {
    // Extra octet.
    assert_eq!(
        parse_ping_target("10.0.2.2.5"),
        Err(PingArgError::Invalid("10.0.2.2.5"))
    );
    // Missing octet.
    assert_eq!(
        parse_ping_target("10.0.2"),
        Err(PingArgError::Invalid("10.0.2"))
    );
    // Out-of-range octet.
    assert_eq!(
        parse_ping_target("10.0.2.999"),
        Err(PingArgError::Invalid("10.0.2.999"))
    );
    // Trailing garbage attached to the token itself.
    assert_eq!(
        parse_ping_target("10.0.2.2abc"),
        Err(PingArgError::Invalid("10.0.2.2abc"))
    );
}

// ---------------------------------------------------------------------------
// probe_driver_name
// ---------------------------------------------------------------------------

#[test]
fn test_probe_driver_name_no_driver_registered() {
    let names = ["nic:rtl8139", "nic:intel_nic"];
    assert_eq!(probe_driver_name(&names, |_| false), None);
}

#[test]
fn test_probe_driver_name_finds_first_registered() {
    let names = ["nic:rtl8139", "nic:intel_nic"];
    // Only the second name is "registered".
    assert_eq!(
        probe_driver_name(&names, |n| n == "nic:intel_nic"),
        Some("nic:intel_nic")
    );
}

#[test]
fn test_probe_driver_name_prefers_first_when_multiple_match() {
    let names = ["nic:rtl8139", "nic:intel_nic"];
    // Both would match; probe order must still win with the first entry.
    assert_eq!(probe_driver_name(&names, |_| true), Some("nic:rtl8139"));
}

// ---------------------------------------------------------------------------
// format_arp_table
// ---------------------------------------------------------------------------

#[test]
fn test_format_arp_table_empty() {
    let status = sample_status(0, 1);
    assert_eq!(format_arp_table(&status), "ARP table is empty.\n");
}

#[test]
fn test_format_arp_table_single_entry() {
    let status = sample_status(1, 1);
    let formatted = format_arp_table(&status);
    assert!(formatted.starts_with("Address                  HWaddress\n"));
    assert_eq!(formatted.lines().count(), 2);
    assert!(formatted.contains("10.0.2.0"));
}

#[test]
fn test_format_arp_table_max_entries() {
    let status = sample_status(MAX_ARP_ENTRIES as u32, 1);
    let formatted = format_arp_table(&status);
    // One header line + one line per entry.
    assert_eq!(formatted.lines().count(), 1 + MAX_ARP_ENTRIES);
    assert!(formatted.contains("10.0.2.0"));
    assert!(formatted.contains(&alloc::format!("10.0.2.{}", MAX_ARP_ENTRIES - 1)));
}

// ---------------------------------------------------------------------------
// format_ifconfig
// ---------------------------------------------------------------------------

#[test]
fn test_format_ifconfig_link_up() {
    let status = sample_status(0, 1);
    let formatted = format_ifconfig(&status);
    assert!(formatted.contains("10.0.2.15"));
    assert!(formatted.contains("10.0.2.2"));
    assert!(formatted.contains("255.255.255.0"));
    assert!(formatted.contains("8.8.8.8"));
    assert!(formatted.contains("link         : up"));
    assert!(formatted.contains("RX Packets   : 42 (4242 bytes)"));
    assert!(formatted.contains("TX Packets   : 24 (2424 bytes)"));
}

#[test]
fn test_format_ifconfig_link_down() {
    let status = sample_status(0, 0);
    let formatted = format_ifconfig(&status);
    assert!(formatted.contains("link         : down"));
}

// ---------------------------------------------------------------------------
// ping_loss_percent
// ---------------------------------------------------------------------------

#[test]
fn test_ping_loss_percent_partial_loss() {
    // 3 of 4 replies received -> 25% loss, not 24 or 26.
    assert_eq!(ping_loss_percent(4, 3), 25);
}

#[test]
fn test_ping_loss_percent_no_loss() {
    assert_eq!(ping_loss_percent(4, 4), 0);
}

#[test]
fn test_ping_loss_percent_total_loss() {
    assert_eq!(ping_loss_percent(4, 0), 100);
}

#[test]
fn test_ping_loss_percent_nothing_transmitted() {
    assert_eq!(ping_loss_percent(0, 0), 0);
}
