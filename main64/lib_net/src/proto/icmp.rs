//! Internet Control Message Protocol (ICMP) Echo Request and Echo Reply handling.

use super::ipv4::compute_checksum;

/// Minimum ICMP header length for Echo Request/Reply (8 bytes).
pub const ICMP_HEADER_LEN: usize = 8;

/// ICMP message types.
pub mod msg_type {
    /// Echo Reply (ping response).
    pub const ECHO_REPLY: u8 = 0;
    /// Echo Request (ping request).
    pub const ECHO_REQUEST: u8 = 8;
}

/// Parsed or constructed ICMP Echo packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcmpEchoPacket<'a> {
    /// ICMP type (0 for Echo Reply, 8 for Echo Request).
    pub icmp_type: u8,
    /// ICMP code (always 0 for Echo).
    pub code: u8,
    /// Checksum from packet.
    pub checksum: u16,
    /// Echo identifier.
    pub identifier: u16,
    /// Echo sequence number.
    pub sequence_number: u16,
    /// Echo payload data.
    pub payload: &'a [u8],
}

impl<'a> IcmpEchoPacket<'a> {
    /// Constructs a new ICMP Echo Request.
    pub fn build_echo_request(identifier: u16, sequence_number: u16, payload: &'a [u8]) -> Self {
        Self {
            icmp_type: msg_type::ECHO_REQUEST,
            code: 0,
            checksum: 0,
            identifier,
            sequence_number,
            payload,
        }
    }

    /// Constructs a new ICMP Echo Reply.
    pub fn build_echo_reply(identifier: u16, sequence_number: u16, payload: &'a [u8]) -> Self {
        Self {
            icmp_type: msg_type::ECHO_REPLY,
            code: 0,
            checksum: 0,
            identifier,
            sequence_number,
            payload,
        }
    }

    /// Parses an ICMP packet from `data`.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        // Step 1: Enforce minimum ICMP Echo header length.
        if data.len() < ICMP_HEADER_LEN {
            return None;
        }

        // Step 2: Validate ICMP checksum over the entire packet.
        let checksum = compute_checksum(data);
        if checksum != 0 {
            return None;
        }

        let icmp_type = data[0];
        let code = data[1];
        let raw_checksum = u16::from_be_bytes([data[2], data[3]]);
        let identifier = u16::from_be_bytes([data[4], data[5]]);
        let sequence_number = u16::from_be_bytes([data[6], data[7]]);
        let payload = &data[ICMP_HEADER_LEN..];

        Some(Self {
            icmp_type,
            code,
            checksum: raw_checksum,
            identifier,
            sequence_number,
            payload,
        })
    }

    /// Serializes this ICMP packet into `buf`, computing the correct checksum.
    ///
    /// Returns `Some(bytes_written)` or `None` if `buf` is too small.
    pub fn serialize(&self, buf: &mut [u8]) -> Option<usize> {
        let total_len = ICMP_HEADER_LEN + self.payload.len();
        if buf.len() < total_len {
            return None;
        }

        buf[0] = self.icmp_type;
        buf[1] = self.code;
        buf[2..4].copy_from_slice(&[0, 0]); // Zero checksum for calculation
        buf[4..6].copy_from_slice(&self.identifier.to_be_bytes());
        buf[6..8].copy_from_slice(&self.sequence_number.to_be_bytes());
        buf[ICMP_HEADER_LEN..total_len].copy_from_slice(self.payload);

        let csum = compute_checksum(&buf[0..total_len]);
        buf[2..4].copy_from_slice(&csum.to_be_bytes());

        Some(total_len)
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
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
}
