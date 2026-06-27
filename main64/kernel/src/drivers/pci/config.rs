//! PCI configuration space port I/O operations.

use crate::arch::port::PortLong;
use crate::drivers::pci::types::{BarType, PciBar};

/// Standard PCI Configuration Address Port.
pub const PCI_CONFIG_ADDRESS: u16 = 0xCF8;

/// Standard PCI Configuration Data Port.
pub const PCI_CONFIG_DATA: u16 = 0xCFC;

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
pub fn read_bar(bus: u8, slot: u8, func: u8, bar_index: usize) -> PciBar {
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
                    bar_type: BarType::Memory64 {
                        address,
                        size,
                        prefetchable,
                    },
                    raw_value: original,
                }
            } else {
                // Invalid state: 64-bit Memory BAR at BAR5. Fall back to 32-bit.
                let address = original & 0xFFFFFFF0;
                let size = if size_mask == 0 {
                    0
                } else {
                    !(size_mask & 0xFFFFFFF0) + 1
                };

                PciBar {
                    bar_type: BarType::Memory32 {
                        address,
                        size,
                        prefetchable,
                    },
                    raw_value: original,
                }
            }
        } else {
            // Step 5d: 32-bit Memory BAR.
            let address = original & 0xFFFFFFF0;
            let size = if size_mask == 0 {
                0
            } else {
                !(size_mask & 0xFFFFFFF0) + 1
            };

            PciBar {
                bar_type: BarType::Memory32 {
                    address,
                    size,
                    prefetchable,
                },
                raw_value: original,
            }
        }
    }
}
