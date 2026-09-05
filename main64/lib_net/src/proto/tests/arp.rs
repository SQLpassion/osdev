use super::*;

#[test]
fn test_ipv4_parse_and_display() {
    let ip = Ipv4Address::parse_str("10.0.2.15").expect("valid IPv4 string");
    assert_eq!(ip.octets(), [10, 0, 2, 15]);

    assert!(Ipv4Address::parse_str("256.0.0.1").is_none());
    assert!(Ipv4Address::parse_str("10.0.2").is_none());
    assert!(Ipv4Address::parse_str("10.0.2.15.1").is_none());
    assert!(Ipv4Address::parse_str("abc").is_none());
}

#[test]
fn test_arp_request_build_and_parse() {
    let sender_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let sender_ip = Ipv4Address::new(10, 0, 2, 15);
    let target_ip = Ipv4Address::new(10, 0, 2, 2);

    let req = ArpPacket::build_request(sender_mac, sender_ip, target_ip);
    assert_eq!(req.opcode, opcode::REQUEST);
    assert_eq!(req.target_mac, MacAddress::ZERO);

    let mut buf = [0u8; 64];
    let written = req.serialize(&mut buf).expect("serialize ARP request");
    assert_eq!(written, 28);

    let parsed = ArpPacket::parse(&buf[..written]).expect("parse ARP request");
    assert_eq!(parsed.sender_mac, sender_mac);
    assert_eq!(parsed.sender_ip, sender_ip);
    assert_eq!(parsed.target_ip, target_ip);
    assert_eq!(parsed.opcode, opcode::REQUEST);
}

#[test]
fn test_arp_parse_rejects_non_ethernet_ipv4_types() {
    let sender_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let sender_ip = Ipv4Address::new(10, 0, 2, 15);
    let target_ip = Ipv4Address::new(10, 0, 2, 2);
    let req = ArpPacket::build_request(sender_mac, sender_ip, target_ip);

    let mut buf = [0u8; 64];
    let written = req.serialize(&mut buf).expect("serialize ARP request");

    // A packet with hardware_len=6/protocol_len=4 but a wrong hardware_type
    // must still be rejected, even though the length fields alone look valid.
    let mut wrong_hw_type = buf;
    wrong_hw_type[0..2].copy_from_slice(&6u16.to_be_bytes());
    assert!(ArpPacket::parse(&wrong_hw_type[..written]).is_none());

    // Likewise for a wrong protocol_type.
    let mut wrong_proto_type = buf;
    wrong_proto_type[2..4].copy_from_slice(&0x0806u16.to_be_bytes());
    assert!(ArpPacket::parse(&wrong_proto_type[..written]).is_none());
}

#[test]
fn test_arp_table_lookup_and_update() {
    let mut table = ArpTable::new();
    let ip = Ipv4Address::new(10, 0, 2, 2);
    let mac1 = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    let mac2 = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x99]);

    assert_eq!(table.lookup(ip), None);
    table.update(ip, mac1);
    assert_eq!(table.lookup(ip), Some(mac1));

    // Update existing entry
    table.update(ip, mac2);
    assert_eq!(table.lookup(ip), Some(mac2));
    assert_eq!(table.entries().len(), 1);
}

#[test]
fn test_arp_table_evicts_oldest_entry_once_full() {
    let mut table = ArpTable::new();
    let mac_for = |n: u8| MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, n]);

    for i in 0..MAX_ENTRIES {
        table.update(Ipv4Address::new(10, 0, 0, i as u8), mac_for(i as u8));
    }
    assert_eq!(table.entries().len(), MAX_ENTRIES);

    let first_ip = Ipv4Address::new(10, 0, 0, 0);
    assert_eq!(table.lookup(first_ip), Some(mac_for(0)));

    // One more distinct IP should evict the oldest entry rather than grow
    // the table past MAX_ENTRIES.
    let new_ip = Ipv4Address::new(10, 0, 1, 0);
    table.update(new_ip, mac_for(200));

    assert_eq!(table.entries().len(), MAX_ENTRIES);
    assert_eq!(
        table.lookup(first_ip),
        None,
        "oldest entry should be evicted"
    );
    assert_eq!(table.lookup(new_ip), Some(mac_for(200)));
}

#[test]
fn test_ipv4_is_same_subnet() {
    let mask = Ipv4Address::new(255, 255, 255, 0);
    let ip1 = Ipv4Address::new(192, 168, 1, 50);
    let ip2 = Ipv4Address::new(192, 168, 1, 1);
    let ip_diff = Ipv4Address::new(10, 0, 2, 15);

    assert!(ip1.is_same_subnet(ip2, mask));
    assert!(!ip1.is_same_subnet(ip_diff, mask));
}
