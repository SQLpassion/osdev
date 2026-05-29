//! PCI Subsystem Integration Tests
//!
//! Verifies the correct initialization and device scanning behaviors of the PCI bus driver.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use kaos_kernel::drivers::pci;
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the PCI integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

// ============================================================================
// Integration Tests
// ============================================================================

/// Contract: PCI initialization and device scanning.
/// Given: The memory subsystem (PMM, VMM, Heap) is fully initialized.
/// When: The PCI init method is called to perform a bus scan, and devices are retrieved.
/// Then: The PCI scan completes, and all found devices (if any) satisfy valid slot and function bounds, and vendor/device IDs are valid.
/// Failure Impact: Indicates a regression in PCI bus scanning or configuration register decoding.
#[test_case]
fn test_pci_init_and_scan() {
    // Step 1: Scan the PCI bus.
    pci::init();

    // Step 2: Retrieve the list of detected devices.
    let devices = pci::get_devices();
    
    // Step 3: Print scanned devices for serial/log inspectability.
    pci::print_devices();

    // Step 4: Validate constraints on all discovered devices.
    for dev in devices.iter() {
        assert!(dev.device < 32, "Device/slot ID must be less than 32");
        assert!(dev.function < 8, "Function ID must be less than 8");
        assert_ne!(dev.vendor_id, 0xFFFF, "Found device with invalid Vendor ID (0xFFFF)");
        assert_ne!(dev.vendor_id, 0x0000, "Found device with invalid Vendor ID (0x0000)");
    }
}

/// Contract: PCI device lookup query by Vendor and Device ID.
/// Given: A completed PCI bus scan.
/// When: Finding a device by Vendor and Device ID.
/// Then: The lookup function either returns None, or a matching device structure.
/// Failure Impact: Indicates a regression in PCI device list query logic.
#[test_case]
fn test_pci_find_device() {
    pci::init();
    let devices = pci::get_devices();

    if let Some(first_device) = devices.first() {
        // If a device exists under QEMU, find_device must successfully retrieve it.
        let found = pci::find_device(first_device.vendor_id, first_device.device_id);
        assert!(found.is_some(), "Should find the registered device");
        let found_dev = found.unwrap();
        assert_eq!(found_dev.bus, first_device.bus);
        assert_eq!(found_dev.device, first_device.device);
        assert_eq!(found_dev.function, first_device.function);
    } else {
        // Otherwise, lookup for non-existent device must return None.
        let found = pci::find_device(0x1234, 0x5678);
        assert!(found.is_none(), "Should not find non-existent device");
    }
}

/// Contract: PCI device lookup query by Class and Subclass.
/// Given: A completed PCI bus scan.
/// When: Finding a device by its class and subclass codes.
/// Then: The lookup function either returns None, or a matching device structure.
/// Failure Impact: Indicates a regression in PCI class/subclass lookup logic.
#[test_case]
fn test_pci_find_by_class() {
    pci::init();
    let devices = pci::get_devices();

    if let Some(first_device) = devices.first() {
        // Class/subclass lookup must succeed for any discovered device.
        let found = pci::find_by_class(first_device.class_code, first_device.subclass);
        assert!(found.is_some(), "Should find a device with matching class/subclass");
        let found_dev = found.unwrap();
        assert_eq!(found_dev.class_code, first_device.class_code);
        assert_eq!(found_dev.subclass, first_device.subclass);
    } else {
        // Lookup on dummy values must return None.
        let found = pci::find_by_class(0xFF, 0xFF);
        assert!(found.is_none(), "Should not find device with class 0xFF");
    }
}
