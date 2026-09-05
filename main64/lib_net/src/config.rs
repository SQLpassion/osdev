//! Network interface configuration.

use super::proto::arp::Ipv4Address;
use super::proto::ethernet::MacAddress;

/// Network configuration settings for the local host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkConfig {
    /// Hardware MAC address of the network interface.
    pub mac: MacAddress,
    /// Local IPv4 address (default: 192.168.1.200).
    pub ip: Ipv4Address,
    /// Gateway IPv4 address (default: 192.168.1.1).
    pub gateway: Ipv4Address,
    /// Subnet mask (default: 255.255.255.0).
    pub subnet_mask: Ipv4Address,
    /// DNS server address (default: 192.168.1.1).
    pub dns: Ipv4Address,
}

impl NetworkConfig {
    /// Creates a network configuration using default parameters (192.168.1.0/24).
    pub fn default_qemu(mac: MacAddress) -> Self {
        Self {
            mac,
            ip: Ipv4Address::new(192, 168, 1, 200),
            gateway: Ipv4Address::new(192, 168, 1, 1),
            subnet_mask: Ipv4Address::new(255, 255, 255, 0),
            dns: Ipv4Address::new(192, 168, 1, 3),
        }
    }
}
