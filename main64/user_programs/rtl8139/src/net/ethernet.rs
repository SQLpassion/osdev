//! Ethernet II frame parsing, construction, and MAC address operations.

use core::fmt;

/// Standard Ethernet header length (6 bytes DST + 6 bytes SRC + 2 bytes EtherType).
pub const ETHERNET_HEADER_LEN: usize = 14;

/// Minimum Ethernet payload length (padded to 46 bytes to reach 60 bytes minimum frame without FCS).
pub const MIN_ETHERNET_FRAME_LEN: usize = 60;

/// Standard EtherType identifiers.
pub mod ethertype {
    /// IPv4 protocol packet.
    pub const IPV4: u16 = 0x0800;
    /// Address Resolution Protocol (ARP).
    pub const ARP: u16 = 0x0806;
}

/// 48-bit Media Access Control (MAC) address.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    /// Broadcast MAC address (`FF:FF:FF:FF:FF:FF`).
    pub const BROADCAST: Self = Self([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

    /// Unspecified all-zero MAC address (`00:00:00:00:00:00`).
    pub const ZERO: Self = Self([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    /// Creates a new `MacAddress` from 6 raw bytes.
    #[inline]
    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    /// Returns the raw 6 bytes of this MAC address.
    #[inline]
    pub const fn bytes(&self) -> [u8; 6] {
        self.0
    }

    /// Returns `true` if this address is the broadcast address.
    #[inline]
    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    }

    /// Returns `true` if this address is a multicast address (least significant bit of first octet is 1).
    #[inline]
    pub fn is_multicast(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }

    /// Returns `true` if this address is all zeros.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0 == [0, 0, 0, 0, 0, 0]
    }
}

impl fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// Parsed view of an Ethernet II frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthernetFrame<'a> {
    /// Destination MAC address.
    pub dest_mac: MacAddress,
    /// Source MAC address.
    pub src_mac: MacAddress,
    /// EtherType protocol identifier (e.g. 0x0800 for IPv4, 0x0806 for ARP).
    pub ethertype: u16,
    /// Payload slice following the 14-byte Ethernet header.
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Parses an Ethernet II frame from a byte buffer.
    ///
    /// Returns `None` if `data.len() < 14`.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        // Step 1: Reject runt frames shorter than standard header.
        if data.len() < ETHERNET_HEADER_LEN {
            return None;
        }

        // Step 2: Extract destination MAC (bytes 0..6).
        let mut dest = [0u8; 6];
        dest.copy_from_slice(&data[0..6]);

        // Step 3: Extract source MAC (bytes 6..12).
        let mut src = [0u8; 6];
        src.copy_from_slice(&data[6..12]);

        // Step 4: Extract big-endian EtherType (bytes 12..14).
        let ethertype = u16::from_be_bytes([data[12], data[13]]);

        // Step 5: Payload slice spans remaining bytes.
        let payload = &data[ETHERNET_HEADER_LEN..];

        Some(Self {
            dest_mac: MacAddress(dest),
            src_mac: MacAddress(src),
            ethertype,
            payload,
        })
    }

    /// Serializes this Ethernet frame into `buf`.
    ///
    /// Returns `Some(bytes_written)` or `None` if `buf` is too small.
    pub fn serialize(&self, buf: &mut [u8]) -> Option<usize> {
        let total_len = ETHERNET_HEADER_LEN + self.payload.len();
        if buf.len() < total_len {
            return None;
        }

        // Step 1: Write Destination MAC.
        buf[0..6].copy_from_slice(&self.dest_mac.0);

        // Step 2: Write Source MAC.
        buf[6..12].copy_from_slice(&self.src_mac.0);

        // Step 3: Write EtherType in big-endian network byte order.
        let et_bytes = self.ethertype.to_be_bytes();
        buf[12..14].copy_from_slice(&et_bytes);

        // Step 4: Copy payload.
        buf[ETHERNET_HEADER_LEN..total_len].copy_from_slice(self.payload);

        Some(total_len)
    }
}

#[cfg(test)]
mod tests {
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
}
