use super::*;

#[test]
fn test_mac_address_display_and_properties() {
    let mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    assert_eq!(mac.bytes(), [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    assert!(!mac.is_broadcast());
    assert!(!mac.is_zero());
    assert!(!mac.is_multicast());

    assert!(MacAddress::BROADCAST.is_broadcast());
    assert!(MacAddress::BROADCAST.is_multicast());
    assert!(MacAddress::ZERO.is_zero());
}

#[test]
fn test_ethernet_frame_serialization_and_parsing() {
    let src = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let dest = MacAddress::BROADCAST;
    let payload = b"Hello KAOS Net!";

    let frame = EthernetFrame {
        dest_mac: dest,
        src_mac: src,
        ethertype: ethertype::IPV4,
        payload,
    };

    let mut buffer = [0u8; 128];
    let written = frame.serialize(&mut buffer).expect("serialize frame");
    assert_eq!(written, 14 + payload.len());

    let parsed = EthernetFrame::parse(&buffer[..written]).expect("parse frame");
    assert_eq!(parsed.dest_mac, dest);
    assert_eq!(parsed.src_mac, src);
    assert_eq!(parsed.ethertype, ethertype::IPV4);
    assert_eq!(parsed.payload, payload);
}

#[test]
fn test_ethernet_frame_rejects_truncated() {
    let short_data = [0u8; 13];
    assert!(EthernetFrame::parse(&short_data).is_none());
}
