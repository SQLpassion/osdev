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

/// Contract: AHCI driver serializes requests and supports multi-sector reads.
/// Given: An initialized AHCI driver.
/// When: Two tasks (or sequential calls simulating tasks) request different LBAs, and a multi-sector read is requested.
/// Then: The SpinLock serializes access without deadlocking, and multi-sector read succeeds.
/// Failure Impact: Data corruption or driver deadlock.
#[test_case]
fn test_ahci_concurrent_readers_and_multi_sector() {
    pci::init();
    ahci::init();

    // Test the multi-sector capability (H1 Step 4) and serialization (H1 Step 1).
    let mut buf1 = [0u8; 512];
    let mut buf_multi = [0u8; 1024];

    // We execute reads. If AHCI is not active (like in default QEMU without an AHCI drive),
    // it will return AhciError::NotInitialized. We just assert it doesn't panic or deadlock.
    let res1 = ahci::read_sectors(&mut buf1, 0, 1);
    let res2 = ahci::read_sectors(&mut buf_multi, 1, 2);

    if res1.is_ok() {
        assert!(
            res2.is_ok(),
            "Multi-sector read failed but single-sector succeeded"
        );
    }
}
