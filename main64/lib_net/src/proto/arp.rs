//! Address Resolution Protocol (ARP) packet parsing, generation, and dynamic ARP cache.

extern crate alloc;
use alloc::vec::Vec;
use core::fmt;

use super::ethernet::MacAddress;

/// Standard ARP packet length for Ethernet + IPv4.
pub const ARP_PACKET_LEN: usize = 28;

/// Hardware type for Ethernet (1).
pub const HARDWARE_TYPE_ETHERNET: u16 = 1;

/// Protocol type for IPv4 (0x0800).
pub const PROTOCOL_TYPE_IPV4: u16 = 0x0800;

/// ARP operation codes.
pub mod opcode {
    /// ARP Request (who has IP X? Tell Y).
    pub const REQUEST: u16 = 1;
    /// ARP Reply (IP X is at MAC Y).
    pub const REPLY: u16 = 2;
}

/// 32-bit IPv4 network address.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    /// Broadcast IPv4 address (`255.255.255.255`).
    pub const BROADCAST: Self = Self([255, 255, 255, 255]);

    /// Unspecified all-zero IPv4 address (`0.0.0.0`).
    pub const ZERO: Self = Self([0, 0, 0, 0]);

    /// Creates a new IPv4 address from 4 octets.
    #[inline]
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    /// Returns the raw 4-byte array representation.
    #[inline]
    pub const fn octets(&self) -> [u8; 4] {
        self.0
    }

    /// Returns `true` if this address is all zeros (`0.0.0.0`).
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0 == [0, 0, 0, 0]
    }

    /// Parses an IPv4 address from a dotted-decimal string (e.g. `"10.0.2.2"`).
    pub fn parse_str(s: &str) -> Option<Self> {
        let mut octets = [0u8; 4];
        let mut octet_idx = 0;
        let mut current_val: u32 = 0;
        let mut has_digits = false;

        for &b in s.as_bytes() {
            if b == b'.' {
                if !has_digits || octet_idx >= 3 || current_val > 255 {
                    return None;
                }
                octets[octet_idx] = current_val as u8;
                octet_idx += 1;
                current_val = 0;
                has_digits = false;
            } else if b.is_ascii_digit() {
                has_digits = true;
                current_val = current_val
                    .checked_mul(10)?
                    .checked_add((b - b'0') as u32)?;
                if current_val > 255 {
                    return None;
                }
            } else {
                return None;
            }
        }

        if !has_digits || octet_idx != 3 || current_val > 255 {
            return None;
        }
        octets[3] = current_val as u8;

        Some(Self(octets))
    }

    /// Checks if this address and `other` belong to the same subnet according to `mask`.
    #[inline]
    pub fn is_same_subnet(&self, other: Self, mask: Self) -> bool {
        for i in 0..4 {
            if (self.0[i] & mask.0[i]) != (other.0[i] & mask.0[i]) {
                return false;
            }
        }
        true
    }
}

impl fmt::Debug for Ipv4Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

impl fmt::Display for Ipv4Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

/// Parsed structure of an Ethernet / IPv4 ARP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArpPacket {
    /// Hardware type (usually 1 for Ethernet).
    pub hardware_type: u16,
    /// Protocol type (usually 0x0800 for IPv4).
    pub protocol_type: u16,
    /// Hardware address length (6 for MAC).
    pub hardware_len: u8,
    /// Protocol address length (4 for IPv4).
    pub protocol_len: u8,
    /// Operation code: 1 for Request, 2 for Reply.
    pub opcode: u16,
    /// Sender hardware (MAC) address.
    pub sender_mac: MacAddress,
    /// Sender protocol (IPv4) address.
    pub sender_ip: Ipv4Address,
    /// Target hardware (MAC) address.
    pub target_mac: MacAddress,
    /// Target protocol (IPv4) address.
    pub target_ip: Ipv4Address,
}

impl ArpPacket {
    /// Constructs a standard ARP Request packet.
    pub fn build_request(
        sender_mac: MacAddress,
        sender_ip: Ipv4Address,
        target_ip: Ipv4Address,
    ) -> Self {
        Self {
            hardware_type: HARDWARE_TYPE_ETHERNET,
            protocol_type: PROTOCOL_TYPE_IPV4,
            hardware_len: 6,
            protocol_len: 4,
            opcode: opcode::REQUEST,
            sender_mac,
            sender_ip,
            target_mac: MacAddress::ZERO,
            target_ip,
        }
    }

    /// Constructs a standard ARP Reply packet.
    pub fn build_reply(
        sender_mac: MacAddress,
        sender_ip: Ipv4Address,
        target_mac: MacAddress,
        target_ip: Ipv4Address,
    ) -> Self {
        Self {
            hardware_type: HARDWARE_TYPE_ETHERNET,
            protocol_type: PROTOCOL_TYPE_IPV4,
            hardware_len: 6,
            protocol_len: 4,
            opcode: opcode::REPLY,
            sender_mac,
            sender_ip,
            target_mac,
            target_ip,
        }
    }

    /// Parses an ARP packet from a byte buffer.
    pub fn parse(data: &[u8]) -> Option<Self> {
        // Step 1: Enforce minimum ARP payload length.
        if data.len() < ARP_PACKET_LEN {
            return None;
        }

        // Step 2: Validate Ethernet (1) and IPv4 (0x0800) fields.
        let hardware_type = u16::from_be_bytes([data[0], data[1]]);
        let protocol_type = u16::from_be_bytes([data[2], data[3]]);
        let hardware_len = data[4];
        let protocol_len = data[5];
        let opcode = u16::from_be_bytes([data[6], data[7]]);

        if hardware_type != HARDWARE_TYPE_ETHERNET
            || protocol_type != PROTOCOL_TYPE_IPV4
            || hardware_len != 6
            || protocol_len != 4
        {
            return None;
        }

        // Step 3: Extract MAC and IP fields.
        let mut s_mac = [0u8; 6];
        s_mac.copy_from_slice(&data[8..14]);
        let mut s_ip = [0u8; 4];
        s_ip.copy_from_slice(&data[14..18]);

        let mut t_mac = [0u8; 6];
        t_mac.copy_from_slice(&data[18..24]);
        let mut t_ip = [0u8; 4];
        t_ip.copy_from_slice(&data[24..28]);

        Some(Self {
            hardware_type,
            protocol_type,
            hardware_len,
            protocol_len,
            opcode,
            sender_mac: MacAddress(s_mac),
            sender_ip: Ipv4Address(s_ip),
            target_mac: MacAddress(t_mac),
            target_ip: Ipv4Address(t_ip),
        })
    }

    /// Serializes this ARP packet into `buf`.
    ///
    /// Returns `Some(bytes_written)` or `None` if `buf` is too small.
    pub fn serialize(&self, buf: &mut [u8]) -> Option<usize> {
        if buf.len() < ARP_PACKET_LEN {
            return None;
        }

        buf[0..2].copy_from_slice(&self.hardware_type.to_be_bytes());
        buf[2..4].copy_from_slice(&self.protocol_type.to_be_bytes());
        buf[4] = self.hardware_len;
        buf[5] = self.protocol_len;
        buf[6..8].copy_from_slice(&self.opcode.to_be_bytes());

        buf[8..14].copy_from_slice(&self.sender_mac.0);
        buf[14..18].copy_from_slice(&self.sender_ip.0);
        buf[18..24].copy_from_slice(&self.target_mac.0);
        buf[24..28].copy_from_slice(&self.target_ip.0);

        Some(ARP_PACKET_LEN)
    }
}

/// Maximum number of entries retained in the ARP cache. Without a cap, a flood of
/// ARP packets carrying distinct spoofed sender IPs would grow this table without
/// limit and exhaust the driver task's memory.
const MAX_ENTRIES: usize = 128;

/// Dynamic ARP cache storing IP to MAC address mappings.
#[derive(Debug, Default)]
pub struct ArpTable {
    entries: Vec<(Ipv4Address, MacAddress)>,
}

impl ArpTable {
    /// Creates a new empty ARP table.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Looks up a MAC address corresponding to `ip`.
    pub fn lookup(&self, ip: Ipv4Address) -> Option<MacAddress> {
        for (entry_ip, entry_mac) in &self.entries {
            if *entry_ip == ip {
                return Some(*entry_mac);
            }
        }
        None
    }

    /// Updates or inserts an ARP entry for `(ip, mac)`.
    ///
    /// When the table is already at [`MAX_ENTRIES`] and `ip` is not yet cached, the
    /// oldest entry is evicted to make room, bounding the table's memory footprint.
    pub fn update(&mut self, ip: Ipv4Address, mac: MacAddress) {
        // Step 1: Update existing mapping if IP is already cached.
        for (entry_ip, entry_mac) in &mut self.entries {
            if *entry_ip == ip {
                *entry_mac = mac;
                return;
            }
        }

        // Step 2: Evict the oldest entry if the cache is full, then append the new mapping.
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push((ip, mac));
    }

    /// Returns a slice of all cached ARP entries.
    pub fn entries(&self) -> &[(Ipv4Address, MacAddress)] {
        &self.entries
    }

    /// Clears all entries from the ARP table.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
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
        assert_eq!(table.lookup(first_ip), None, "oldest entry should be evicted");
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
}
