use super::*;

#[test]
fn test_compute_checksum_rfc_vectors() {
    let test_data = [
        0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00, 0x40, 0x06, 0x00, 0x00, 0xac, 0x10, 0x0a,
        0x63, 0xac, 0x10, 0x0a, 0x0c,
    ];
    let csum = compute_checksum(&test_data);
    assert_eq!(csum, 0xb1e6);

    // Verification over entire header with checksum included must equal 0
    let mut verified_header = test_data;
    verified_header[10..12].copy_from_slice(&csum.to_be_bytes());
    assert_eq!(compute_checksum(&verified_header), 0);
}

#[test]
fn test_ipv4_header_serialization_and_parsing() {
    let src = Ipv4Address::new(10, 0, 2, 15);
    let dest = Ipv4Address::new(10, 0, 2, 2);
    let payload = b"ICMP Echo Payload";

    let mut packet_buf = [0u8; 128];
    let mut header = [0u8; 20];
    Ipv4Packet::serialize_header(
        src,
        dest,
        protocol::ICMP,
        payload.len(),
        0x1234,
        DEFAULT_TTL,
        &mut header,
    );

    packet_buf[0..20].copy_from_slice(&header);
    packet_buf[20..20 + payload.len()].copy_from_slice(payload);

    let total_len = 20 + payload.len();
    let parsed = Ipv4Packet::parse(&packet_buf[..total_len]).expect("parse valid IPv4 packet");

    assert_eq!(parsed.version, 4);
    assert_eq!(parsed.ihl, 5);
    assert_eq!(parsed.src_ip, src);
    assert_eq!(parsed.dest_ip, dest);
    assert_eq!(parsed.protocol, protocol::ICMP);
    assert_eq!(parsed.payload, payload);
}

#[test]
fn test_ipv4_corrupted_checksum_rejected() {
    let src = Ipv4Address::new(10, 0, 2, 15);
    let dest = Ipv4Address::new(10, 0, 2, 2);
    let mut header = [0u8; 20];
    Ipv4Packet::serialize_header(src, dest, protocol::ICMP, 0, 1, 64, &mut header);

    // Corrupt header byte
    header[8] ^= 0xFF;
    assert!(Ipv4Packet::parse(&header).is_none());
}
