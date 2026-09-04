//! Network stack protocol coordinator handling Ethernet, ARP, IPv4, and ICMP.

use super::config::NetworkConfig;
use super::event::NetworkEvent;
use super::proto::arp::{opcode as arp_opcode, ArpPacket, ArpTable, Ipv4Address};
use super::proto::ethernet::{ethertype, EthernetFrame, MacAddress};
use super::proto::icmp::{msg_type as icmp_type, IcmpEchoPacket};
use super::proto::ipv4::{protocol as ip_protocol, Ipv4Packet, DEFAULT_TTL};

/// Integrated `#![no_std]` network protocol coordinator.
pub struct NetworkStack {
    /// Active network configuration.
    pub config: NetworkConfig,
    /// ARP resolution table.
    pub arp_table: ArpTable,
    /// Total received packets count.
    pub rx_packets: usize,
    /// Total transmitted packets count.
    pub tx_packets: usize,
    /// Total received bytes count.
    pub rx_bytes: usize,
    /// Total transmitted bytes count.
    pub tx_bytes: usize,
}

impl NetworkStack {
    /// Creates a new network stack for the given hardware MAC address.
    pub fn new(mac: MacAddress) -> Self {
        Self {
            config: NetworkConfig::default_qemu(mac),
            arp_table: ArpTable::new(),
            rx_packets: 0,
            tx_packets: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }

    /// Handles an incoming raw Ethernet frame from the RX ring.
    ///
    /// Automatically replies to ARP requests targeting our IP and ICMP echo requests.
    pub fn handle_rx_packet<F>(&mut self, packet_data: &[u8], tx_fn: F) -> NetworkEvent
    where
        F: FnMut(&[u8]),
    {
        self.rx_packets += 1;
        self.rx_bytes += packet_data.len();

        // Step 1: Parse Ethernet II frame.
        let Some(eth) = EthernetFrame::parse(packet_data) else {
            return NetworkEvent::None;
        };

        // Step 2: MAC filtering (accept broadcast or destination matching our MAC).
        if !eth.dest_mac.is_broadcast() && eth.dest_mac != self.config.mac {
            return NetworkEvent::None;
        }

        // Step 3: Demultiplex by EtherType.
        match eth.ethertype {
            ethertype::ARP => self.process_arp(eth.src_mac, eth.payload, tx_fn),
            ethertype::IPV4 => self.process_ipv4(eth.payload, tx_fn),
            _ => NetworkEvent::None,
        }
    }

    /// Processes an incoming ARP packet.
    fn process_arp<F>(&mut self, _eth_src: MacAddress, payload: &[u8], mut tx_fn: F) -> NetworkEvent
    where
        F: FnMut(&[u8]),
    {
        let Some(arp) = ArpPacket::parse(payload) else {
            return NetworkEvent::None;
        };

        // Always cache sender IP and MAC in the ARP table
        self.arp_table.update(arp.sender_ip, arp.sender_mac);

        if arp.opcode == arp_opcode::REQUEST {
            // Step 1: If ARP request targets our IP, send ARP reply.
            if arp.target_ip == self.config.ip {
                let reply_arp = ArpPacket::build_reply(
                    self.config.mac,
                    self.config.ip,
                    arp.sender_mac,
                    arp.sender_ip,
                );

                let mut arp_buf = [0u8; 28];
                if let Some(arp_len) = reply_arp.serialize(&mut arp_buf) {
                    let eth_reply = EthernetFrame {
                        dest_mac: arp.sender_mac,
                        src_mac: self.config.mac,
                        ethertype: ethertype::ARP,
                        payload: &arp_buf[..arp_len],
                    };

                    let mut out_frame = [0u8; 60]; // Minimum Ethernet frame
                    if let Some(frame_len) = eth_reply.serialize(&mut out_frame) {
                        let final_len = frame_len.max(60);
                        self.tx_packets += 1;
                        self.tx_bytes += final_len;
                        tx_fn(&out_frame[..final_len]);
                    }
                }
                NetworkEvent::ArpRequestAnswered {
                    sender_ip: arp.sender_ip,
                    sender_mac: arp.sender_mac,
                }
            } else {
                NetworkEvent::None
            }
        } else if arp.opcode == arp_opcode::REPLY {
            NetworkEvent::ArpReplyReceived {
                sender_ip: arp.sender_ip,
                sender_mac: arp.sender_mac,
            }
        } else {
            NetworkEvent::None
        }
    }

    /// Processes an incoming IPv4 packet.
    fn process_ipv4<F>(&mut self, payload: &[u8], mut tx_fn: F) -> NetworkEvent
    where
        F: FnMut(&[u8]),
    {
        let Some(ip) = Ipv4Packet::parse(payload) else {
            return NetworkEvent::None;
        };

        // Filter destination IP
        if ip.dest_ip != self.config.ip && ip.dest_ip != Ipv4Address::BROADCAST {
            return NetworkEvent::None;
        }

        if ip.protocol == ip_protocol::ICMP {
            let Some(icmp) = IcmpEchoPacket::parse(ip.payload) else {
                return NetworkEvent::None;
            };

            if icmp.icmp_type == icmp_type::ECHO_REQUEST {
                // Step 1: Automatically respond to ICMP Echo Request.
                let reply_icmp = IcmpEchoPacket::build_echo_reply(
                    icmp.identifier,
                    icmp.sequence_number,
                    icmp.payload,
                );

                let mut icmp_buf = [0u8; 1500];
                if let Some(icmp_len) = reply_icmp.serialize(&mut icmp_buf) {
                    let mut ip_hdr = [0u8; 20];
                    Ipv4Packet::serialize_header(
                        self.config.ip,
                        ip.src_ip,
                        ip_protocol::ICMP,
                        icmp_len,
                        0x2000,
                        DEFAULT_TTL,
                        &mut ip_hdr,
                    );

                    let mut ip_packet = [0u8; 1520];
                    ip_packet[0..20].copy_from_slice(&ip_hdr);
                    ip_packet[20..20 + icmp_len].copy_from_slice(&icmp_buf[..icmp_len]);
                    let ip_packet_len = 20 + icmp_len;

                    // Resolve destination MAC
                    let dest_mac = self
                        .arp_table
                        .lookup(ip.src_ip)
                        .unwrap_or(MacAddress::BROADCAST);

                    let eth_frame = EthernetFrame {
                        dest_mac,
                        src_mac: self.config.mac,
                        ethertype: ethertype::IPV4,
                        payload: &ip_packet[..ip_packet_len],
                    };

                    let mut out_frame = [0u8; 1536];
                    if let Some(frame_len) = eth_frame.serialize(&mut out_frame) {
                        let final_len = frame_len.max(60);
                        self.tx_packets += 1;
                        self.tx_bytes += final_len;
                        tx_fn(&out_frame[..final_len]);
                    }
                }

                NetworkEvent::IcmpEchoRequestAnswered {
                    src_ip: ip.src_ip,
                    sequence: icmp.sequence_number,
                }
            } else if icmp.icmp_type == icmp_type::ECHO_REPLY {
                NetworkEvent::IcmpEchoReply {
                    src_ip: ip.src_ip,
                    identifier: icmp.identifier,
                    sequence: icmp.sequence_number,
                    ttl: ip.ttl,
                    data_len: icmp.payload.len(),
                }
            } else {
                NetworkEvent::None
            }
        } else {
            NetworkEvent::None
        }
    }

    /// Constructs an outgoing ARP Request packet inside `out_buf`.
    pub fn build_arp_request(&self, target_ip: Ipv4Address, out_buf: &mut [u8]) -> Option<usize> {
        let arp = ArpPacket::build_request(self.config.mac, self.config.ip, target_ip);
        let mut arp_payload = [0u8; 28];
        let arp_len = arp.serialize(&mut arp_payload)?;

        let eth = EthernetFrame {
            dest_mac: MacAddress::BROADCAST,
            src_mac: self.config.mac,
            ethertype: ethertype::ARP,
            payload: &arp_payload[..arp_len],
        };

        let written = eth.serialize(out_buf)?;
        Some(written.max(60))
    }

    /// Constructs an outgoing ICMP Echo Request frame targeting `target_ip`.
    pub fn build_ping(
        &self,
        target_ip: Ipv4Address,
        dest_mac: MacAddress,
        identifier: u16,
        sequence: u16,
        payload: &[u8],
        out_buf: &mut [u8],
    ) -> Option<usize> {
        let icmp = IcmpEchoPacket::build_echo_request(identifier, sequence, payload);
        let mut icmp_buf = [0u8; 1500];
        let icmp_len = icmp.serialize(&mut icmp_buf)?;

        let mut ip_hdr = [0u8; 20];
        Ipv4Packet::serialize_header(
            self.config.ip,
            target_ip,
            ip_protocol::ICMP,
            icmp_len,
            sequence,
            DEFAULT_TTL,
            &mut ip_hdr,
        );

        let mut ip_packet = [0u8; 1520];
        ip_packet[0..20].copy_from_slice(&ip_hdr);
        ip_packet[20..20 + icmp_len].copy_from_slice(&icmp_buf[..icmp_len]);
        let ip_len = 20 + icmp_len;

        let eth = EthernetFrame {
            dest_mac,
            src_mac: self.config.mac,
            ethertype: ethertype::IPV4,
            payload: &ip_packet[..ip_len],
        };

        let written = eth.serialize(out_buf)?;
        Some(written.max(60))
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/stack.rs"]
mod tests;
