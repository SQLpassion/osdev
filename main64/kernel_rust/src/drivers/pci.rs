//! PCI (Peripheral Component Interconnect) Bus Driver
//!
//! Exposes structures and functions to scan the PCI bus, retrieve configuration
//! space registers, and query discovered devices (e.g., for AHCI or Network interfaces).

#![allow(dead_code)]

use crate::arch::port::PortLong;
use crate::debugln;
use crate::sync::spinlock::SpinLock;
use alloc::vec::Vec;

/// Standard PCI Configuration Address Port.
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;

/// Standard PCI Configuration Data Port.
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// Global list of scanned PCI devices, protected by a SpinLock.
static PCI_DEVICES: SpinLock<Vec<PciDevice>> = SpinLock::new(Vec::new());

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

/// Read a 32-bit double word from the PCI configuration space.
///
/// # Safety
/// This operation uses Port I/O which is hardware-sensitive and inherently unsafe.
pub unsafe fn pci_config_read(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let address = (1 << 31)
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset & 0xFC) as u32);
    
    let address_port = PortLong::new(PCI_CONFIG_ADDRESS);
    let data_port = PortLong::new(PCI_CONFIG_DATA);

    // SAFETY:
    // - Writing to the PCI address port and reading from the data port is safe
    //   when the CPU is in Ring 0 and interrupts are disabled or synchronized.
    // - The address is formed strictly in accordance with the PCI Local Bus Specification.
    unsafe {
        address_port.write(address);
        data_port.read()
    }
}

/// Write a 32-bit double word to the PCI configuration space.
///
/// # Safety
/// This operation uses Port I/O which is hardware-sensitive and inherently unsafe.
pub unsafe fn pci_config_write(bus: u8, slot: u8, func: u8, offset: u8, val: u32) {
    let address = (1 << 31)
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset & 0xFC) as u32);
    
    let address_port = PortLong::new(PCI_CONFIG_ADDRESS);
    let data_port = PortLong::new(PCI_CONFIG_DATA);

    // SAFETY:
    // - Writing to the PCI address and data ports is safe when the CPU is in
    //   Ring 0 and interrupts are disabled or synchronized.
    // - The address is formed strictly in accordance with the PCI Local Bus Specification.
    unsafe {
        address_port.write(address);
        data_port.write(val);
    }
}

/// Read a 16-bit word from the PCI configuration space.
///
/// # Safety
/// Accesses hardware via Port I/O.
pub unsafe fn pci_config_read_u16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    // SAFETY:
    // - Delegates to pci_config_read, which requires caller verification of Port I/O safety.
    let val = unsafe { pci_config_read(bus, slot, func, offset) };
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Read an 8-bit byte from the PCI configuration space.
///
/// # Safety
/// Accesses hardware via Port I/O.
pub unsafe fn pci_config_read_u8(bus: u8, slot: u8, func: u8, offset: u8) -> u8 {
    // SAFETY:
    // - Delegates to pci_config_read, which requires caller verification of Port I/O safety.
    let val = unsafe { pci_config_read(bus, slot, func, offset) };
    ((val >> ((offset & 3) * 8)) & 0xFF) as u8
}

/// Reads and determines the size of a Base Address Register (BAR) for a PCI device.
///
/// # Safety
/// This function temporarily writes all 1s to the BAR register in PCI configuration space
/// to determine the size of the address space it requests, and then restores the original value.
/// This must be done at boot time or before the BAR is mapped.
fn read_bar(bus: u8, slot: u8, func: u8, bar_index: usize) -> PciBar {
    let offset = 0x10 + (bar_index as u8) * 4;

    // Step 1: Read the original BAR register value.
    // SAFETY:
    // - Accessing configuration space is safe here, but uses raw port operations internally.
    let original = unsafe { pci_config_read(bus, slot, func, offset) };
    if original == 0 {
        return PciBar {
            bar_type: BarType::None,
            raw_value: 0,
        };
    }

    // Step 2: Write all 1s to the BAR to request the sizing mask from the hardware.
    // SAFETY:
    // - Writing 0xFFFFFFFF to a BAR is the standard way to size the BAR according to the PCI spec.
    // - We immediately restore the original value, ensuring hardware state consistency.
    unsafe { pci_config_write(bus, slot, func, offset, 0xFFFFFFFF) };

    // Step 3: Read back the sizing mask.
    // SAFETY:
    // - Standard PCI configuration read operation.
    let size_mask = unsafe { pci_config_read(bus, slot, func, offset) };

    // Step 4: Restore the original BAR value.
    // SAFETY:
    // - Crucial step to keep the device configuration in its original/valid state.
    unsafe { pci_config_write(bus, slot, func, offset, original) };

    // Step 5: Parse BAR type and calculate the size.
    if (original & 1) == 1 {
        // Step 5a: Decode I/O space BAR.
        let port = (original & 0xFFFC) as u16;
        let size = if size_mask == 0 {
            0
        } else {
            (!(size_mask & 0xFFFC) + 1) & 0xFFFF
        };

        PciBar {
            bar_type: BarType::Io { port, size },
            raw_value: original,
        }
    } else {
        // Step 5b: Decode Memory space BAR (either 32-bit or 64-bit).
        let prefetchable = (original & 0x08) != 0;
        let type_bits = (original >> 1) & 0x03;

        if type_bits == 2 {
            // Step 5c: 64-bit Memory BAR. Next BAR holds upper 32 bits.
            if bar_index < 5 {
                let next_offset = offset + 4;
                
                // Read next BAR original value.
                // SAFETY:
                // - Standard PCI configuration read.
                let next_original = unsafe { pci_config_read(bus, slot, func, next_offset) };

                // Write 1s to next BAR to size the upper 32 bits.
                // SAFETY:
                // - Standard PCI sizing procedure.
                unsafe { pci_config_write(bus, slot, func, next_offset, 0xFFFFFFFF) };
                let next_size_mask = unsafe { pci_config_read(bus, slot, func, next_offset) };
                unsafe { pci_config_write(bus, slot, func, next_offset, next_original) };

                let address = ((next_original as u64) << 32) | ((original & 0xFFFFFFF0) as u64);
                let full_mask = ((next_size_mask as u64) << 32) | ((size_mask & 0xFFFFFFF0) as u64);
                let size = if full_mask == 0 { 0 } else { !full_mask + 1 };

                PciBar {
                    bar_type: BarType::Memory64 { address, size, prefetchable },
                    raw_value: original,
                }
            } else {
                // Invalid state: 64-bit Memory BAR at BAR5. Fall back to 32-bit.
                let address = original & 0xFFFFFFF0;
                let size = if size_mask == 0 { 0 } else { !(size_mask & 0xFFFFFFF0) + 1 };

                PciBar {
                    bar_type: BarType::Memory32 { address, size, prefetchable },
                    raw_value: original,
                }
            }
        } else {
            // Step 5d: 32-bit Memory BAR.
            let address = original & 0xFFFFFFF0;
            let size = if size_mask == 0 { 0 } else { !(size_mask & 0xFFFFFFF0) + 1 };

            PciBar {
                bar_type: BarType::Memory32 { address, size, prefetchable },
                raw_value: original,
            }
        }
    }
}

/// Initialize the PCI subsystem by scanning all buses and devices.
pub fn init() {
    // Step 1: Clear the global device list to ensure idempotency.
    let mut devices = PCI_DEVICES.lock();
    devices.clear();

    // Step 2: Perform a brute force scan of all 256 PCI buses.
    for bus in 0..=255 {
        // Step 3: Scan all 32 slots (devices) on the current bus.
        for slot in 0..32 {
            // Step 4: Read function 0 first to check if a device exists.
            // SAFETY:
            // - Reading PCI configuration space registers is safe as it only retrieves metadata.
            let vendor_id = unsafe { pci_config_read_u16(bus, slot, 0, 0x00) };
            if vendor_id == 0xFFFF || vendor_id == 0x0000 {
                // Device does not exist, skip it.
                continue;
            }

            // Step 5: Read header type to check if it's a multi-function device.
            // SAFETY:
            // - Reading offset 0x0C is safe as it is a standard PCI configuration register.
            let header_type = unsafe { pci_config_read_u8(bus, slot, 0, 0x0E) };
            let max_functions = if (header_type & 0x80) != 0 { 8 } else { 1 };

            // Step 6: Scan all functions of this device.
            for func in 0..max_functions {
                // SAFETY:
                // - Reading PCI configuration space registers is safe.
                let func_vendor_id = unsafe { pci_config_read_u16(bus, slot, func, 0x00) };
                if func_vendor_id == 0xFFFF || func_vendor_id == 0x0000 {
                    continue;
                }

                // Step 7: Gather all metadata for the PCI device.
                let device_id = unsafe { pci_config_read_u16(bus, slot, func, 0x02) };
                let class_code = unsafe { pci_config_read_u8(bus, slot, func, 0x0B) };
                let subclass = unsafe { pci_config_read_u8(bus, slot, func, 0x0A) };
                let prog_if = unsafe { pci_config_read_u8(bus, slot, func, 0x09) };
                let revision_id = unsafe { pci_config_read_u8(bus, slot, func, 0x08) };
                let interrupt_line = unsafe { pci_config_read_u8(bus, slot, func, 0x3C) };
                let interrupt_pin = unsafe { pci_config_read_u8(bus, slot, func, 0x3D) };

                // Step 8: Parse all Base Address Registers (BARs).
                let mut bars = [PciBar { bar_type: BarType::None, raw_value: 0 }; 6];
                let mut skip_next = false;
                for (bar_idx, bar_slot) in bars.iter_mut().enumerate() {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }

                    // Read the BAR details and size it.
                    let bar = read_bar(bus, slot, func, bar_idx);
                    *bar_slot = bar;

                    // If it's a 64-bit Memory BAR, we need to skip the next slot as it's part of the same BAR.
                    if let BarType::Memory64 { .. } = bar.bar_type {
                        skip_next = true;
                    }
                }

                // Step 9: Push the discovered device to the global list.
                devices.push(PciDevice {
                    bus,
                    device: slot,
                    function: func,
                    vendor_id: func_vendor_id,
                    device_id,
                    class_code,
                    subclass,
                    prog_if,
                    revision_id,
                    header_type,
                    interrupt_line,
                    interrupt_pin,
                    bars,
                });
            }
        }
    }
}

/// Return a copy of all scanned PCI devices.
pub fn get_devices() -> Vec<PciDevice> {
    PCI_DEVICES.lock().clone()
}

/// Find a PCI device by Vendor ID and Device ID.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices.iter().find(|d| d.vendor_id == vendor_id && d.device_id == device_id).cloned()
}

/// Find a PCI device by Class and Subclass.
pub fn find_by_class(class_code: u8, subclass: u8) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices.iter().find(|d| d.class_code == class_code && d.subclass == subclass).cloned()
}

/// Filter a slice of PCI devices based on a category filter string.
/// Supported filters (case-insensitive):
/// - "--ethernet", "--network" -> Class 0x02
/// - "--storage", "--sata", "--ide" -> Class 0x01
/// - "--display", "--vga" -> Class 0x03
/// - "--bridge" -> Class 0x06
pub fn filter_by_category(devices: &[PciDevice], filter: &str) -> Vec<PciDevice> {
    let filter_lower = filter.trim();
    devices.iter().filter(|d| {
        // Class 0x02 corresponds to Network Controller
        if filter_lower.eq_ignore_ascii_case("--ethernet") || filter_lower.eq_ignore_ascii_case("--network") {
            d.class_code == 0x02
        // Class 0x01 corresponds to Mass Storage Controller
        } else if filter_lower.eq_ignore_ascii_case("--storage") || filter_lower.eq_ignore_ascii_case("--sata") || filter_lower.eq_ignore_ascii_case("--ide") {
            d.class_code == 0x01
        // Class 0x03 corresponds to Display Controller
        } else if filter_lower.eq_ignore_ascii_case("--display") || filter_lower.eq_ignore_ascii_case("--vga") {
            d.class_code == 0x03
        // Class 0x06 corresponds to Bridge Device
        } else if filter_lower.eq_ignore_ascii_case("--bridge") {
            d.class_code == 0x06
        } else {
            false
        }
    }).cloned().collect()
}

/// Print all discovered PCI devices to the screen or debug output.
pub fn print_devices() {
    let devices = PCI_DEVICES.lock();
    debugln!("--- PCI Bus Scan ({} devices found) ---", devices.len());
    for dev in devices.iter() {
        let vendor_name = vendor_to_str(dev.vendor_id);
        let device_name = device_to_str(dev.vendor_id, dev.device_id);
        let class_name = class_to_str(dev.class_code, dev.subclass);

        debugln!(
            "PCI {:02x}:{:02x}.{} | {} ({:04x}): {} ({:04x}) | {} | IRQ Line {}",
            dev.bus,
            dev.device,
            dev.function,
            vendor_name,
            dev.vendor_id,
            device_name,
            dev.device_id,
            class_name,
            dev.interrupt_line
        );
        for (i, bar) in dev.bars.iter().enumerate() {
            match bar.bar_type {
                BarType::Io { port, size } => {
                    debugln!("  BAR {}: I/O Port {:#x} (size {})", i, port, size);
                }
                BarType::Memory32 { address, size, prefetchable } => {
                    debugln!(
                        "  BAR {}: 32-bit Memory {:#010x} (size {}, prefetchable: {})",
                        i,
                        address,
                        size,
                        prefetchable
                    );
                }
                BarType::Memory64 { address, size, prefetchable } => {
                    debugln!(
                        "  BAR {}: 64-bit Memory {:#018x} (size {}, prefetchable: {})",
                        i,
                        address,
                        size,
                        prefetchable
                    );
                }
                BarType::None => {}
            }
        }
    }
}

/// Helper to map a Class and Subclass code to a human-readable name.
pub fn class_to_str(class: u8, subclass: u8) -> &'static str {
    match class {
        0x00 => "Unclassified Device",
        0x01 => match subclass {
            0x00 => "SCSI Storage Controller",
            0x01 => "IDE Interface",
            0x02 => "Floppy Disk Controller",
            0x03 => "IPI Bus Controller",
            0x04 => "RAID Controller",
            0x05 => "ATA Controller",
            0x06 => "SATA Controller",
            0x07 => "Serial Attached SCSI (SAS) Controller",
            0x08 => "Non-Volatile Memory (NVM/NVMe) Controller",
            _ => "Mass Storage Controller",
        },
        0x02 => match subclass {
            0x00 => "Ethernet Controller",
            0x01 => "Token Ring Controller",
            0x02 => "FDDI Controller",
            0x03 => "ATM Controller",
            0x04 => "ISDN Controller",
            0x05 => "WorldFip Controller",
            0x06 => "PICMG Multi-computing",
            _ => "Network Controller",
        },
        0x03 => match subclass {
            0x00 => "VGA Compatible Controller",
            0x01 => "XGA Controller",
            0x02 => "3D Controller",
            _ => "Display Controller",
        },
        0x04 => "Multimedia Device",
        0x05 => "Memory Controller",
        0x06 => match subclass {
            0x00 => "Host Bridge",
            0x01 => "ISA Bridge",
            0x02 => "EISA Bridge",
            0x03 => "MCA Bridge",
            0x04 => "PCI-to-PCI Bridge",
            0x05 => "PCMCIA Bridge",
            0x06 => "NuBus Bridge",
            0x07 => "CardBus Bridge",
            0x08 => "Semi-Transparent PCI-to-PCI Bridge",
            _ => "Bridge Device",
        },
        0x07 => "Simple Communication Controller",
        0x08 => "Base System Peripheral",
        0x09 => "Input Device Controller",
        0x0A => "Docking Station",
        0x0B => "Processor",
        0x0C => match subclass {
            0x00 => "FireWire (IEEE 1394) Controller",
            0x01 => "ACCESS.bus Controller",
            0x02 => "SSA Controller",
            0x03 => "USB Controller",
            0x04 => "Fibre Channel Controller",
            0x05 => "System Management Bus (SMBus)",
            _ => "Serial Bus Controller",
        },
        0x0D => "Wireless Controller",
        0x0E => "Intelligent Controller",
        0x0F => "Satellite Communications Controller",
        0x10 => "Encryption Controller",
        0x11 => "Signal Processing Controller",
        _ => "Unknown Class",
    }
}

/// Helper to map a Vendor ID to a human-readable name.
pub fn vendor_to_str(vendor_id: u16) -> &'static str {
    match vendor_id {
        0x8086 => "Intel Corporation",
        0x10EC => "Realtek Semiconductor Co., Ltd.",
        0x10DE => "NVIDIA Corporation",
        0x1002 => "Advanced Micro Devices, Inc. [AMD/ATI]",
        0x1234 => "QEMU/Bochs",
        0x15AD => "VMware",
        0x80EE => "Oracle Corporation (VirtualBox)",
        0x1AF4 => "Red Hat, Inc. (Virtio)",
        0x1014 => "IBM",
        0x1022 => "AMD",
        0x1106 => "VIA Technologies, Inc.",
        0x106b => "Apple Inc.",
        _ => "Unknown Vendor",
    }
}

/// Helper to map a specific Device ID (associated with a Vendor ID) to a human-readable name.
pub fn device_to_str(vendor_id: u16, device_id: u16) -> &'static str {
    match vendor_id {
        0x8086 => match device_id {
            0x1237 => "430FX - 82437FX System Controller [Triton I]",
            0x7000 => "82371SB PIIX3 ISA [Triton II]",
            0x7010 => "82371SB PIIX3 IDE [Triton II]",
            0x7113 => "82371AB/EB/MB PIIX4 ACPI",
            0x100E => "82540EM Gigabit Ethernet Controller",
            0x2922 => "82801IR/ICH9R SATA Controller [AHCI mode]",
            0x2829 => "82801HM/HEM (ICH8M/ICH8M-E) SATA Controller [AHCI mode]",
            _ => "Generic Intel Device",
        },
        0x1234 => match device_id {
            0x1111 => "Bochs/QEMU VGA Card",
            _ => "Generic QEMU/Bochs Device",
        },
        0x1AF4 => match device_id {
            0x1000 => "Virtio Network Card",
            0x1001 => "Virtio Block Device",
            0x1002 => "Virtio Balloon",
            0x1003 => "Virtio Console",
            _ => "Generic Virtio Device",
        },
        _ => "Generic PCI Device",
    }
}
