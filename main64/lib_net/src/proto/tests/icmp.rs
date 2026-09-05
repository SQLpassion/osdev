use super::*;

#[test]
fn test_icmp_echo_request_build_and_parse() {
    let payload = b"0123456789abcdef";
    let req = IcmpEchoPacket::build_echo_request(0xCAFE, 1, payload);

    let mut buffer = [0u8; 64];
    let written = req.serialize(&mut buffer).expect("serialize ICMP");
    assert_eq!(written, ICMP_HEADER_LEN + payload.len());

    let parsed = IcmpEchoPacket::parse(&buffer[..written]).expect("parse ICMP");
    assert_eq!(parsed.icmp_type, msg_type::ECHO_REQUEST);
    assert_eq!(parsed.code, 0);
    assert_eq!(parsed.identifier, 0xCAFE);
    assert_eq!(parsed.sequence_number, 1);
    assert_eq!(parsed.payload, payload);
}

#[test]
fn test_icmp_corrupted_checksum_rejected() {
    let payload = b"Test";
    let req = IcmpEchoPacket::build_echo_request(0x1234, 1, payload);
    let mut buffer = [0u8; 32];
    let written = req.serialize(&mut buffer).unwrap();

    buffer[5] ^= 0xFF; // Corrupt
    assert!(IcmpEchoPacket::parse(&buffer[..written]).is_none());
}
