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
use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::pci;
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
        assert_ne!(
            dev.vendor_id, 0xFFFF,
            "Found device with invalid Vendor ID (0xFFFF)"
        );
        assert_ne!(
            dev.vendor_id, 0x0000,
            "Found device with invalid Vendor ID (0x0000)"
        );
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
        assert!(
            found.is_some(),
            "Should find a device with matching class/subclass"
        );
        let found_dev = found.unwrap();
        assert_eq!(found_dev.class_code, first_device.class_code);
        assert_eq!(found_dev.subclass, first_device.subclass);
    } else {
        // Lookup on dummy values must return None.
        let found = pci::find_by_class(0xFF, 0xFF);
        assert!(found.is_none(), "Should not find device with class 0xFF");
    }
}

/// Contract: PCI human-readable static string translation helpers.
/// Given: Specific standardized Vendor IDs, Device IDs, and Class/Subclass codes.
/// When: Invoking pci::vendor_to_str, pci::device_to_str, or pci::class_to_str.
/// Then: The returned strings match their standardized hardware names.
/// Failure Impact: Indicates a regression in PCI vendor/device/class description lookups.
#[test_case]
fn test_pci_string_mapping() {
    // 1. Verify standard vendors
    assert_eq!(pci::vendor_to_str(0x8086), "Intel Corporation");
    assert_eq!(pci::vendor_to_str(0x1234), "QEMU/Bochs");
    assert_eq!(pci::vendor_to_str(0x9999), "Unknown Vendor");

    // 2. Verify standard devices
    assert_eq!(
        pci::device_to_str(0x8086, 0x100E),
        "82540EM Gigabit Ethernet Controller"
    );
    assert_eq!(pci::device_to_str(0x1234, 0x1111), "Bochs/QEMU VGA Card");
    assert_eq!(pci::device_to_str(0xFFFF, 0x0000), "Generic PCI Device");

    // 3. Verify standard classes
    assert_eq!(pci::class_to_str(0x02, 0x00), "Ethernet Controller");
    assert_eq!(pci::class_to_str(0x03, 0x00), "VGA Compatible Controller");
    assert_eq!(pci::class_to_str(0xFF, 0xFF), "Unknown Class");
}

/// Contract: PCI category filtering helper.
/// Given: A slice of populated PciDevice objects.
/// When: Calling filter_by_category with standard flags (--ethernet, --storage, --display, --bridge).
/// Then: Only the devices matching the specified category's class codes are returned.
/// Failure Impact: Indicates a regression in category-based device filtering.
#[test_case]
fn test_pci_filter_by_category() {
    let dev_storage = pci::PciDevice {
        bus: 0,
        device: 1,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x7010,
        class_code: 0x01, // Storage
        subclass: 0x01,
        prog_if: 0,
        revision_id: 0,
        header_type: 0,
        interrupt_line: 0,
        interrupt_pin: 0,
        bars: [pci::PciBar {
            bar_type: pci::BarType::None,
            raw_value: 0,
        }; 6],
    };

    let dev_network = pci::PciDevice {
        bus: 0,
        device: 2,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x100E,
        class_code: 0x02, // Network
        subclass: 0x00,
        prog_if: 0,
        revision_id: 0,
        header_type: 0,
        interrupt_line: 0,
        interrupt_pin: 0,
        bars: [pci::PciBar {
            bar_type: pci::BarType::None,
            raw_value: 0,
        }; 6],
    };

    let dev_display = pci::PciDevice {
        bus: 0,
        device: 3,
        function: 0,
        vendor_id: 0x1234,
        device_id: 0x1111,
        class_code: 0x03, // Display
        subclass: 0x00,
        prog_if: 0,
        revision_id: 0,
        header_type: 0,
        interrupt_line: 0,
        interrupt_pin: 0,
        bars: [pci::PciBar {
            bar_type: pci::BarType::None,
            raw_value: 0,
        }; 6],
    };

    let dev_bridge = pci::PciDevice {
        bus: 0,
        device: 4,
        function: 0,
        vendor_id: 0x8086,
        device_id: 0x1237,
        class_code: 0x06, // Bridge
        subclass: 0x00,
        prog_if: 0,
        revision_id: 0,
        header_type: 0,
        interrupt_line: 0,
        interrupt_pin: 0,
        bars: [pci::PciBar {
            bar_type: pci::BarType::None,
            raw_value: 0,
        }; 6],
    };

    let devices = [dev_storage, dev_network, dev_display, dev_bridge];

    // 1. Verify --ethernet / --network (should only return network)
    let net1 = pci::filter_by_category(&devices, "--ethernet");
    assert_eq!(net1.len(), 1);
    assert_eq!(net1[0].device_id, 0x100E);

    let net2 = pci::filter_by_category(&devices, "--network");
    assert_eq!(net2.len(), 1);
    assert_eq!(net2[0].device_id, 0x100E);

    // 2. Verify --storage / --sata / --ide (should only return storage)
    let store1 = pci::filter_by_category(&devices, "--storage");
    assert_eq!(store1.len(), 1);
    assert_eq!(store1[0].device_id, 0x7010);

    // 3. Verify --display / --vga (should only return display)
    let disp1 = pci::filter_by_category(&devices, "--vga");
    assert_eq!(disp1.len(), 1);
    assert_eq!(disp1[0].device_id, 0x1111);

    // 4. Verify --bridge (should only return bridge)
    let bridge1 = pci::filter_by_category(&devices, "--bridge");
    assert_eq!(bridge1.len(), 1);
    assert_eq!(bridge1[0].device_id, 0x1237);

    // 5. Verify invalid filter (should return empty vector)
    let invalid = pci::filter_by_category(&devices, "--invalid");
    assert!(invalid.is_empty());
}

/// Contract: PCI device lookup query by index.
/// Given: A completed PCI bus scan.
/// When: Getting a device by index.
/// Then: The get_device function returns the cloned device or None if out of bounds.
/// Failure Impact: Indicates a regression in index-based PCI device query logic.
#[test_case]
fn test_pci_get_device() {
    pci::init();
    let count = pci::get_devices().len();

    for idx in 0..count {
        let dev = pci::get_device(idx);
        assert!(dev.is_some(), "Should retrieve device at valid index");
    }

    let dev_invalid = pci::get_device(count);
    assert!(
        dev_invalid.is_none(),
        "Should not retrieve device at out-of-bounds index"
    );
}
