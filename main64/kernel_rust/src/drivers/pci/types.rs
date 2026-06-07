//! Standard PCI data structures and enums.

/// Types of Base Address Registers (BARs) that a PCI device can expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarType {
    /// Unused BAR.
    None,

    /// Port-mapped I/O.
    Io {
        /// Base port address.
        port: u16,

        /// Size of the address range in bytes.
        size: u32,
    },

    /// 32-bit Memory-mapped I/O.
    Memory32 {
        /// Base physical memory address.
        address: u32,

        /// Size of the address range in bytes.
        size: u32,

        /// Whether the memory is prefetchable.
        prefetchable: bool,
    },

    /// 64-bit Memory-mapped I/O.
    Memory64 {
        /// Base physical memory address.
        address: u64,

        /// Size of the address range in bytes.
        size: u64,

        /// Whether the memory is prefetchable.
        prefetchable: bool,
    },
}

/// Represents a parsed Base Address Register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBar {
    /// Type and specific details of this BAR.
    pub bar_type: BarType,

    /// Raw unparsed value read from the configuration register.
    pub raw_value: u32,
}

/// Represents a scanned PCI device with its configuration details.
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    /// PCI Bus ID (0 to 255).
    pub bus: u8,

    /// PCI Device/Slot ID (0 to 31).
    pub device: u8,

    /// PCI Function ID (0 to 7).
    pub function: u8,

    /// Vendor Identification number.
    pub vendor_id: u16,

    /// Device Identification number.
    pub device_id: u16,

    /// Device class code (e.g. 0x01 for mass storage, 0x02 for network).
    pub class_code: u8,

    /// Device subclass code (e.g. 0x06 for SATA, 0x00 for Ethernet).
    pub subclass: u8,

    /// Programming Interface of the device.
    pub prog_if: u8,

    /// Revision number of the device.
    pub revision_id: u8,

    /// Header Type configuration byte.
    pub header_type: u8,

    /// Interrupt Line mapped to the device.
    pub interrupt_line: u8,

    /// Interrupt Pin requested by the device.
    pub interrupt_pin: u8,

    /// Up to 6 Base Address Registers for this device.
    pub bars: [PciBar; 6],
}
