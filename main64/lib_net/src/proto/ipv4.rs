//! Internet Protocol Version 4 (IPv4) packet handling and RFC 1071 checksum calculation.

use super::arp::Ipv4Address;

/// Minimum IPv4 header length (with no IP options).
pub const IPV4_HEADER_MIN_LEN: usize = 20;

/// Default Time-To-Live (TTL) value for outgoing packets.
pub const DEFAULT_TTL: u8 = 64;

/// IPv4 protocol numbers.
pub mod protocol {
    /// Internet Control Message Protocol (ICMP).
    pub const ICMP: u8 = 1;
    /// Transmission Control Protocol (TCP).
    pub const TCP: u8 = 6;
    /// User Datagram Protocol (UDP).
    pub const UDP: u8 = 17;
}

/// Computes the 16-bit Internet Checksum according to RFC 1071.
///
/// Sums all 16-bit words in one's complement arithmetic and inverts the result.
pub fn compute_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    // Step 1: Accumulate 16-bit words in big-endian network byte order.
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]);
        sum = sum.wrapping_add(word as u32);
        i += 2;
    }

    // Step 2: Handle trailing odd byte if present.
    if i < data.len() {
        let word = u16::from_be_bytes([data[i], 0]);
        sum = sum.wrapping_add(word as u32);
    }

    // Step 3: Fold 32-bit sum into 16 bits.
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    // Step 4: One's complement bitwise inversion.
    !(sum as u16)
}

/// Parsed IPv4 packet view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ipv4Packet<'a> {
    /// Version (must be 4) and Internet Header Length (IHL >= 5).
    pub version: u8,
    /// Internet Header Length in 32-bit words (e.g. 5 for 20 bytes).
    pub ihl: u8,
    /// Total packet length in bytes (header + payload).
    pub total_length: u16,
    /// Packet identification.
    pub identification: u16,
    /// Time To Live.
    pub ttl: u8,
    /// Transport layer protocol number (e.g. 1 for ICMP).
    pub protocol: u8,
    /// Header checksum from packet.
    pub header_checksum: u16,
    /// Source IPv4 address.
    pub src_ip: Ipv4Address,
    /// Destination IPv4 address.
    pub dest_ip: Ipv4Address,
    /// Packet payload slice.
    pub payload: &'a [u8],
}

impl<'a> Ipv4Packet<'a> {
    /// Parses and validates an IPv4 packet from `data`.
    ///
    /// Verifies version 4, minimum header size, total length bounds, and checksum.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        // Step 1: Enforce minimum IPv4 header length.
        if data.len() < IPV4_HEADER_MIN_LEN {
            return None;
        }

        let ver_ihl = data[0];
        let version = (ver_ihl >> 4) & 0x0F;
        let ihl = ver_ihl & 0x0F;

        if version != 4 || ihl < 5 {
            return None;
        }

        let header_len = (ihl as usize) * 4;
        if data.len() < header_len {
            return None;
        }

        // Step 2: Validate header checksum over the header bytes.
        let checksum = compute_checksum(&data[0..header_len]);
        if checksum != 0 {
            return None;
        }

        let total_length = u16::from_be_bytes([data[2], data[3]]);
        if (total_length as usize) < header_len || data.len() < (total_length as usize) {
            return None;
        }

        let identification = u16::from_be_bytes([data[4], data[5]]);
        let ttl = data[8];
        let protocol = data[9];
        let header_checksum = u16::from_be_bytes([data[10], data[11]]);

        let mut src_bytes = [0u8; 4];
        src_bytes.copy_from_slice(&data[12..16]);
        let mut dest_bytes = [0u8; 4];
        dest_bytes.copy_from_slice(&data[16..20]);

        let payload = &data[header_len..total_length as usize];

        Some(Self {
            version,
            ihl,
            total_length,
            identification,
            ttl,
            protocol,
            header_checksum,
            src_ip: Ipv4Address(src_bytes),
            dest_ip: Ipv4Address(dest_bytes),
            payload,
        })
    }

    /// Serializes a standard 20-byte IPv4 header into `out_header`.
    pub fn serialize_header(
        src_ip: Ipv4Address,
        dest_ip: Ipv4Address,
        protocol: u8,
        payload_len: usize,
        identification: u16,
        ttl: u8,
        out_header: &mut [u8; 20],
    ) {
        let total_len = (IPV4_HEADER_MIN_LEN + payload_len) as u16;

        out_header[0] = 0x45; // Version 4, IHL 5 (20 bytes)
        out_header[1] = 0x00; // DSCP / ECN
        out_header[2..4].copy_from_slice(&total_len.to_be_bytes());
        out_header[4..6].copy_from_slice(&identification.to_be_bytes());
        out_header[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Flags: Don't Fragment
        out_header[8] = ttl;
        out_header[9] = protocol;
        out_header[10..12].copy_from_slice(&[0, 0]); // Zero checksum for calculation
        out_header[12..16].copy_from_slice(&src_ip.0);
        out_header[16..20].copy_from_slice(&dest_ip.0);

        // Calculate and insert checksum
        let csum = compute_checksum(out_header);
        out_header[10..12].copy_from_slice(&csum.to_be_bytes());
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/ipv4.rs"]
mod tests;
