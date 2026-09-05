use super::{
    build_status, format_arp_table, ifconfig_header, parse_command_line, parse_single_ipv4_arg,
    Ipv4ArgError,
};
use lib_net::{Ipv4Address, MacAddress, NetworkStack};

#[test]
fn test_parse_command_line_empty_and_whitespace_only() {
    assert_eq!(parse_command_line(""), None);
    assert_eq!(parse_command_line("   "), None);
    assert_eq!(parse_command_line("\t\n"), None);
}

#[test]
fn test_parse_command_line_command_only() {
    assert_eq!(parse_command_line("help"), Some(("help", "")));
    assert_eq!(parse_command_line("  listen  "), Some(("listen", "")));
}

#[test]
fn test_parse_command_line_command_with_args() {
    assert_eq!(
        parse_command_line("ping 10.0.2.2"),
        Some(("ping", "10.0.2.2"))
    );
    assert_eq!(
        parse_command_line("set ip 192.168.1.5"),
        Some(("set", "ip 192.168.1.5"))
    );
    // Extra internal whitespace before the remainder is trimmed.
    assert_eq!(
        parse_command_line("ping    10.0.2.2"),
        Some(("ping", "10.0.2.2"))
    );
}

#[test]
fn test_parse_single_ipv4_arg_success() {
    assert_eq!(
        parse_single_ipv4_arg("10.0.2.2"),
        Ok(Ipv4Address::new(10, 0, 2, 2))
    );
    // Only the first token matters (mirrors `ping <ip>` ignoring trailing junk).
    assert_eq!(
        parse_single_ipv4_arg("10.0.2.2 ignored"),
        Ok(Ipv4Address::new(10, 0, 2, 2))
    );
}

#[test]
fn test_parse_single_ipv4_arg_missing() {
    assert_eq!(parse_single_ipv4_arg(""), Err(Ipv4ArgError::Missing));
    assert_eq!(parse_single_ipv4_arg("   "), Err(Ipv4ArgError::Missing));
}

#[test]
fn test_parse_single_ipv4_arg_invalid() {
    assert_eq!(
        parse_single_ipv4_arg("not-an-ip"),
        Err(Ipv4ArgError::Invalid("not-an-ip"))
    );
    assert_eq!(
        parse_single_ipv4_arg("999.999.999.999"),
        Err(Ipv4ArgError::Invalid("999.999.999.999"))
    );
}

#[test]
fn test_format_arp_table_empty() {
    assert_eq!(format_arp_table(&[]), "ARP table is empty.\n");
}

#[test]
fn test_format_arp_table_with_entries() {
    let entries = [
        (
            Ipv4Address::new(10, 0, 2, 2),
            MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]),
        ),
        (
            Ipv4Address::new(10, 0, 2, 15),
            MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x0F]),
        ),
    ];
    let formatted = format_arp_table(&entries);
    assert!(formatted.starts_with("Address                  HWaddress\n"));
    assert!(formatted.contains("10.0.2.2"));
    assert!(formatted.contains("10.0.2.15"));
    assert_eq!(formatted.lines().count(), 3);
}

#[test]
fn test_ifconfig_header_without_model() {
    assert_eq!(ifconfig_header("rtl8139", None), "Interface rtl8139:");
}

#[test]
fn test_ifconfig_header_with_model() {
    assert_eq!(
        ifconfig_header("intel_nic", Some("I219-V")),
        "Interface intel_nic (I219-V):"
    );
}

fn make_stack() -> NetworkStack {
    NetworkStack::new(MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]))
}

#[test]
fn test_build_status_maps_config_and_counters() {
    let mut stack = make_stack();
    stack.rx_packets = 42;
    stack.rx_bytes = 4242;
    stack.tx_packets = 24;
    stack.tx_bytes = 2424;

    let status = build_status(&stack);

    assert_eq!(status.mac, stack.config.mac.0);
    assert_eq!(status.ip, stack.config.ip.0);
    assert_eq!(status.subnet_mask, stack.config.subnet_mask.0);
    assert_eq!(status.gateway, stack.config.gateway.0);
    assert_eq!(status.dns, stack.config.dns.0);
    assert_eq!(status.rx_packets, 42);
    assert_eq!(status.rx_bytes, 4242);
    assert_eq!(status.tx_packets, 24);
    assert_eq!(status.tx_bytes, 2424);
    assert_eq!(status.link_up, 1);
    assert_eq!(status.arp_entry_count, 0);
}

#[test]
fn test_build_status_maps_arp_entries() {
    let mut stack = make_stack();
    let ip_a = Ipv4Address::new(10, 0, 2, 2);
    let mac_a = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    let ip_b = Ipv4Address::new(10, 0, 2, 3);
    let mac_b = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x03]);
    stack.arp_table.update(ip_a, mac_a);
    stack.arp_table.update(ip_b, mac_b);

    let status = build_status(&stack);

    assert_eq!(status.arp_entry_count, 2);
    assert_eq!(status.arp_entries[0].ip, ip_a.0);
    assert_eq!(status.arp_entries[0].mac, mac_a.0);
    assert_eq!(status.arp_entries[1].ip, ip_b.0);
    assert_eq!(status.arp_entries[1].mac, mac_b.0);
}

#[test]
fn test_build_status_truncates_arp_table_at_max_entries() {
    let mut stack = make_stack();
    // ArpTable::MAX_ENTRIES (128) is far larger than
    // lib_driver::MAX_ARP_ENTRIES (16) -- populate more than the latter to
    // prove build_status truncates on its own, before DrvPublishStatus ever
    // sees the snapshot (Step 3 rejects an over-count outright).
    let extra_entries = lib_driver::MAX_ARP_ENTRIES + 5;
    for i in 0..extra_entries {
        let ip = Ipv4Address::new(10, 0, 3, i as u8);
        let mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x35, i as u8]);
        stack.arp_table.update(ip, mac);
    }

    let status = build_status(&stack);

    assert_eq!(status.arp_entry_count as usize, lib_driver::MAX_ARP_ENTRIES);
    // The first MAX_ARP_ENTRIES entries (insertion order) must be present,
    // matching ArpTable::entries()'s iteration order.
    for i in 0..lib_driver::MAX_ARP_ENTRIES {
        let expected_ip = Ipv4Address::new(10, 0, 3, i as u8);
        assert_eq!(status.arp_entries[i].ip, expected_ip.0);
    }
}
