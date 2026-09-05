//! Network stack event notifications.

use super::proto::arp::Ipv4Address;
use super::proto::ethernet::MacAddress;

/// Incoming network event processed by the stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkEvent {
    /// Received ARP request targeting our IP and automatically answered.
    ArpRequestAnswered {
        sender_ip: Ipv4Address,
        sender_mac: MacAddress,
    },
    /// Received ARP reply updating the ARP table.
    ArpReplyReceived {
        sender_ip: Ipv4Address,
        sender_mac: MacAddress,
    },
    /// Received ICMP Echo Reply (ping response).
    IcmpEchoReply {
        src_ip: Ipv4Address,
        identifier: u16,
        sequence: u16,
        ttl: u8,
        data_len: usize,
    },
    /// Received ICMP Echo Request and automatically answered.
    IcmpEchoRequestAnswered { src_ip: Ipv4Address, sequence: u16 },
    /// Ignored or non-actionable packet.
    None,
}
