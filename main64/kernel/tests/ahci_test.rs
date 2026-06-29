//! AHCI Subsystem Integration Tests
//!
//! Verifies the initialization of the experimental AHCI driver.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::{ahci, pci};
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the AHCI integration test kernel.
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

/// Contract: AHCI driver initialization handles missing hardware gracefully.
/// Given: A fully initialized subsystem (PCI, PMM, VMM).
/// When: Calling `ahci::init()`.
/// Then: It returns gracefully without crashing, correctly handling the case where AHCI might not be present (e.g. default QEMU IDE).
/// Failure Impact: Indicates a regression or unhandled exception during AHCI initialization.
#[test_case]
fn test_ahci_init_does_not_crash() {
    pci::init();

    // We expect this to run without triple faulting or panicking.
    // In default QEMU without `-device ahci`, it will gracefully log "No controller found".
    // If AHCI is present, it will initialize the MMIO registers.
    ahci::init();
}
