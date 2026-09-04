use super::*;
use crate::nic::NicDevice;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use lib_driver::SysError;

struct MockNic {
    mac: MacAddress,
    rx_queue: VecDeque<Vec<u8>>,
    tx_log: Vec<Vec<u8>>,
}

impl MockNic {
    fn new(mac: MacAddress) -> Self {
        Self {
            mac,
            rx_queue: VecDeque::new(),
            tx_log: Vec::new(),
        }
    }
}

impl NicDevice for MockNic {
    fn mac(&self) -> MacAddress {
        self.mac
    }

    fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError> {
        self.tx_log.push(packet.to_vec());
        Ok(())
    }

    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize> {
        let pkt = self.rx_queue.pop_front()?;
        let n = pkt.len().min(out_buf.len());
        out_buf[..n].copy_from_slice(&pkt[..n]);
        Some(n)
    }

    fn shutdown(&mut self) {}
}

#[test]
fn test_mock_nic_stack_handle_arp_request_and_reply() {
    let my_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let mut nic = MockNic::new(my_mac);
    let mut stack = NetworkStack::new(my_mac);

    let sender_mac = MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let sender_ip = Ipv4Address::new(192, 168, 1, 50);
    let arp_req = ArpPacket::build_request(sender_mac, sender_ip, stack.config.ip);

    let mut arp_bytes = [0u8; 28];
    let arp_len = arp_req.serialize(&mut arp_bytes).unwrap();
    let eth_frame = EthernetFrame {
        dest_mac: MacAddress::BROADCAST,
        src_mac: sender_mac,
        ethertype: ethertype::ARP,
        payload: &arp_bytes[..arp_len],
    };
    let mut eth_buf = [0u8; 60];
    let eth_len = eth_frame.serialize(&mut eth_buf).unwrap();

    let event = stack.handle_rx_packet(&eth_buf[..eth_len], |tx_pkt| {
        nic.transmit(tx_pkt).unwrap();
    });

    assert_eq!(
        event,
        NetworkEvent::ArpRequestAnswered {
            sender_ip,
            sender_mac,
        }
    );
    assert_eq!(nic.tx_log.len(), 1);
    assert_eq!(stack.arp_table.lookup(sender_ip), Some(sender_mac));
}

#[test]
fn test_mock_nic_stack_handle_icmp_echo_request() {
    let my_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let mut nic = MockNic::new(my_mac);
    let mut stack = NetworkStack::new(my_mac);

    let remote_ip = Ipv4Address::new(192, 168, 1, 100);
    let remote_mac = MacAddress::new([0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    stack.arp_table.update(remote_ip, remote_mac);

    let icmp_req = IcmpEchoPacket::build_echo_request(0x1234, 1, b"PingData");
    let mut icmp_bytes = [0u8; 64];
    let icmp_len = icmp_req.serialize(&mut icmp_bytes).unwrap();

    let mut ip_hdr = [0u8; 20];
    Ipv4Packet::serialize_header(
        remote_ip,
        stack.config.ip,
        ip_protocol::ICMP,
        icmp_len,
        1,
        DEFAULT_TTL,
        &mut ip_hdr,
    );
    let mut ip_packet = [0u8; 128];
    ip_packet[0..20].copy_from_slice(&ip_hdr);
    ip_packet[20..20 + icmp_len].copy_from_slice(&icmp_bytes[..icmp_len]);
    let ip_len = 20 + icmp_len;

    let eth_frame = EthernetFrame {
        dest_mac: my_mac,
        src_mac: remote_mac,
        ethertype: ethertype::IPV4,
        payload: &ip_packet[..ip_len],
    };
    let mut eth_buf = [0u8; 128];
    let eth_len = eth_frame.serialize(&mut eth_buf).unwrap();

    let event = stack.handle_rx_packet(&eth_buf[..eth_len], |tx_pkt| {
        nic.transmit(tx_pkt).unwrap();
    });

    assert_eq!(
        event,
        NetworkEvent::IcmpEchoRequestAnswered {
            src_ip: remote_ip,
            sequence: 1,
        }
    );
    assert_eq!(nic.tx_log.len(), 1);
}

#[test]
fn test_icmp_echo_reply_unicasts_to_sender_when_arp_table_has_no_entry() {
    let my_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let mut nic = MockNic::new(my_mac);
    let mut stack = NetworkStack::new(my_mac);

    let remote_ip = Ipv4Address::new(192, 168, 1, 100);
    let remote_mac = MacAddress::new([0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    // Deliberately do NOT populate the ARP table: the sender pings us before
    // we ever learned its MAC via ARP, which is exactly the case that used
    // to fall back to a LAN-wide broadcast reply.
    assert_eq!(stack.arp_table.lookup(remote_ip), None);

    let icmp_req = IcmpEchoPacket::build_echo_request(0x1234, 1, b"PingData");
    let mut icmp_bytes = [0u8; 64];
    let icmp_len = icmp_req.serialize(&mut icmp_bytes).unwrap();

    let mut ip_hdr = [0u8; 20];
    Ipv4Packet::serialize_header(
        remote_ip,
        stack.config.ip,
        ip_protocol::ICMP,
        icmp_len,
        1,
        DEFAULT_TTL,
        &mut ip_hdr,
    );
    let mut ip_packet = [0u8; 128];
    ip_packet[0..20].copy_from_slice(&ip_hdr);
    ip_packet[20..20 + icmp_len].copy_from_slice(&icmp_bytes[..icmp_len]);
    let ip_len = 20 + icmp_len;

    let eth_frame = EthernetFrame {
        dest_mac: my_mac,
        src_mac: remote_mac,
        ethertype: ethertype::IPV4,
        payload: &ip_packet[..ip_len],
    };
    let mut eth_buf = [0u8; 128];
    let eth_len = eth_frame.serialize(&mut eth_buf).unwrap();

    let event = stack.handle_rx_packet(&eth_buf[..eth_len], |tx_pkt| {
        nic.transmit(tx_pkt).unwrap();
    });

    assert_eq!(
        event,
        NetworkEvent::IcmpEchoRequestAnswered {
            src_ip: remote_ip,
            sequence: 1,
        }
    );
    assert_eq!(nic.tx_log.len(), 1);

    let reply = EthernetFrame::parse(&nic.tx_log[0]).expect("reply must be a valid Ethernet frame");
    assert_eq!(
        reply.dest_mac, remote_mac,
        "an Echo Reply to a sender with no ARP entry must unicast to the sender's own \
         Ethernet source address, not broadcast the reply to the whole LAN segment"
    );
}
