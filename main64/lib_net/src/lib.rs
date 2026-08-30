//! Network stack library for KAOS user-mode device drivers and network applications.
//!
//! Provides Ethernet, ARP, IPv4, ICMP, dynamic ARP cache, and the `NicDevice` trait.

#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod config;
pub mod event;
pub mod nic;
pub mod proto;
pub mod stack;

pub use config::NetworkConfig;
pub use event::NetworkEvent;
pub use nic::NicDevice;
pub use proto::arp::{opcode as arp_opcode, ArpPacket, ArpTable, Ipv4Address, ARP_PACKET_LEN};
pub use proto::ethernet::{ethertype, EthernetFrame, MacAddress, ETHERNET_HEADER_LEN};
pub use proto::icmp::{msg_type as icmp_type, IcmpEchoPacket, ICMP_HEADER_LEN};
pub use proto::ipv4::{protocol as ip_protocol, Ipv4Packet, DEFAULT_TTL, IPV4_HEADER_MIN_LEN};
pub use stack::NetworkStack;
