//! PCI (Peripheral Component Interconnect) Bus Driver.
//!
//! Design summary:
//! - Performs brute-force bus scanning across all 256 buses, 32 devices (slots),
//!   and up to 8 functions per device.
//! - Reads and dynamically sizes Base Address Registers (BARs) by temporarily
//!   masking configuration registers with `0xFFFFFFFF` and restoring original states.
//! - Parsed BAR configurations are decoded into standard I/O ports, 32-bit MMIO, or 64-bit MMIO ranges.
//! - Discovered devices are stored inside a global `PCI_DEVICES` registry synchronized
//!   via a thread-safe `SpinLock`.
//! - Provides high-level query utilities by Vendor/Device IDs, Class/Subclass codes, and category filters.
//!
//! Notes:
//! - 64-bit Memory BARs occupy two contiguous BAR slots; the scanning sequence automatically
//!   skips the secondary upper 32-bit slot when a 64-bit mapping is decoded.
//! - Uses translation database modules to map raw vendor/device/class IDs to readable strings.

#![allow(dead_code)]

use alloc::vec::Vec;

use crate::debugln;
use crate::sync::spinlock::SpinLock;

pub mod config;
pub mod database;
pub mod types;

#[allow(unused_imports)]
pub use config::{pci_config_read, pci_config_read_u16, pci_config_read_u8, pci_config_write};
pub use database::{class_to_str, device_to_str, vendor_to_str};
pub use types::{BarType, PciBar, PciDevice};

/// Global list of scanned PCI devices, protected by a SpinLock.
static PCI_DEVICES: SpinLock<Vec<PciDevice>> = SpinLock::new(Vec::new());

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
                let mut bars = [PciBar {
                    bar_type: BarType::None,
                    raw_value: 0,
                }; 6];
                let mut skip_next = false;
                for (bar_idx, bar_slot) in bars.iter_mut().enumerate() {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }

                    // Read the BAR details and size it.
                    let bar = config::read_bar(bus, slot, func, bar_idx);
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

/// Return a copy of a single scanned PCI device.
pub fn get_device(index: usize) -> Option<PciDevice> {
    // Step 1: Lock the global PCI device list.
    let devices = PCI_DEVICES.lock();

    // Step 2: Retrieve and clone only the requested device.
    devices.get(index).cloned()
}

/// Find a PCI device by Vendor ID and Device ID.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices
        .iter()
        .find(|d| d.vendor_id == vendor_id && d.device_id == device_id)
        .cloned()
}

/// Find a PCI device by Class and Subclass.
pub fn find_by_class(class_code: u8, subclass: u8) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices
        .iter()
        .find(|d| d.class_code == class_code && d.subclass == subclass)
        .cloned()
}

/// Filter a slice of PCI devices based on a category filter string.
/// Supported filters (case-insensitive):
/// - "--ethernet", "--network" -> Class 0x02
/// - "--storage", "--sata", "--ide" -> Class 0x01
/// - "--display", "--vga" -> Class 0x03
/// - "--bridge" -> Class 0x06
pub fn filter_by_category(devices: &[PciDevice], filter: &str) -> Vec<PciDevice> {
    let filter_lower = filter.trim();
    devices
        .iter()
        .filter(|d| {
            // Class 0x02 corresponds to Network Controller
            if filter_lower.eq_ignore_ascii_case("--ethernet")
                || filter_lower.eq_ignore_ascii_case("--network")
            {
                d.class_code == 0x02
            // Class 0x01 corresponds to Mass Storage Controller
            } else if filter_lower.eq_ignore_ascii_case("--storage")
                || filter_lower.eq_ignore_ascii_case("--sata")
                || filter_lower.eq_ignore_ascii_case("--ide")
            {
                d.class_code == 0x01
            // Class 0x03 corresponds to Display Controller
            } else if filter_lower.eq_ignore_ascii_case("--display")
                || filter_lower.eq_ignore_ascii_case("--vga")
            {
                d.class_code == 0x03
            // Class 0x06 corresponds to Bridge Device
            } else if filter_lower.eq_ignore_ascii_case("--bridge") {
                d.class_code == 0x06
            } else {
                false
            }
        })
        .cloned()
        .collect()
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
                BarType::Memory32 {
                    address,
                    size,
                    prefetchable,
                } => {
                    debugln!(
                        "  BAR {}: 32-bit Memory {:#010x} (size {}, prefetchable: {})",
                        i,
                        address,
                        size,
                        prefetchable
                    );
                }
                BarType::Memory64 {
                    address,
                    size,
                    prefetchable,
                } => {
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
