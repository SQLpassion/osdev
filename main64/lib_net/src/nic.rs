//! Hardware abstraction for a single Ethernet NIC.

use lib_driver::SysError;

use super::proto::ethernet::MacAddress;

/// Unified interface for all Ethernet device drivers.
///
/// Implementations encapsulate all device-specific register operations,
/// DMA ring management, and interrupt handling. The NetworkStack is
/// generic over this trait so that Ethernet/ARP/IPv4/ICMP exist only once.
pub trait NicDevice {
    /// Returns the hardware-programmed MAC address read from the NIC.
    fn mac(&self) -> MacAddress;

    /// Transmits `packet` as a raw Ethernet frame.
    ///
    /// The caller guarantees `packet.len() >= 14`.
    /// The implementation pads to 60 bytes if required by the hardware.
    fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError>;

    /// Returns the next received Ethernet frame (non-blocking).
    ///
    /// Returns `Some(n)` if a packet was received, otherwise `None`.
    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize>;

    /// Disables TX/RX and releases hardware resources.
    fn shutdown(&mut self);
}
