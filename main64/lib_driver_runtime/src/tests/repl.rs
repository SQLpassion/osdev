use super::{
    format_arp_table, ifconfig_header, parse_command_line, parse_single_ipv4_arg, Ipv4ArgError,
};
use lib_net::{Ipv4Address, MacAddress};

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
