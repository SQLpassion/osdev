//! Block device abstraction integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::block::{self, BlockError};
use kaos_kernel::drivers::{ahci, pci};
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the block device abstraction integration test kernel.
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

/// Contract: block device read/write return NotReady before initialization.
/// Given: The active block device is not set (initial state).
/// When: read_sectors or write_sectors is invoked.
/// Then: They must return BlockError::NotReady.
#[test_case]
fn test_block_uninitialized_returns_not_ready() {
    block::reset_active_device();
    let mut buf = [0u8; 512];
    let read_result = block::read_sectors(0, 1, &mut buf);
    assert!(
        matches!(read_result, Err(BlockError::NotReady)),
        "read_sectors must return NotReady when no device is active"
    );

    let write_result = block::write_sectors(0, 1, &buf);
    assert!(
        matches!(write_result, Err(BlockError::NotReady)),
        "write_sectors must return NotReady when no device is active"
    );
}

/// Contract: block device input validation logic.
/// Given: ATA block device is selected as active.
/// When: read_sectors is called with a buffer smaller than requested count * SECTOR_SIZE.
/// Then: The function must return BlockError::BadBuffer.
#[test_case]
fn test_block_buffer_validation() {
    // Step 1: Initialize ATA device registration in block facade.
    block::init_ata();

    // Step 2: Request reading 2 sectors with a buffer only large enough for 1.
    let mut small_buf = [0u8; 512];
    let read_result = block::read_sectors(0, 2, &mut small_buf);
    assert!(
        matches!(read_result, Err(BlockError::BadBuffer)),
        "read_sectors must fail with BadBuffer if destination is too small"
    );
}

/// Contract: block device LBA validation logic.
/// Given: ATA block device is selected as active.
/// When: read_sectors is called with LBA beyond the 28-bit ATA limit (0x0FFF_FFFF).
/// Then: The function must return BlockError::OutOfRange.
#[test_case]
fn test_block_lba_bounds_checking() {
    // Step 1: Initialize ATA device registration in block facade.
    block::init_ata();

    // Step 2: Attempt to read beyond the device's maximum addressable sector.
    let mut buf = [0u8; 512];
    let result = block::read_sectors(0x1000_0000, 1, &mut buf);
    assert!(
        matches!(result, Err(BlockError::OutOfRange)),
        "read_sectors must reject LBA exceeding device's capacity"
    );
}

/// Contract: AHCI block device write policy.
/// Given: AHCI block device is selected as active but hardware is not initialized.
/// When: write_sectors is invoked.
/// Then: The function must attempt to write and return BlockError::Device (rather than Unsupported).
#[test_case]
fn test_ahci_device_accepts_writes() {
    // Step 1: Initialize AHCI device registration in block facade.
    block::init_ahci();

    // Step 2: Attempt a write operation on the AHCI device.
    let buf = [0u8; 512];
    let result = block::write_sectors(0, 1, &buf);
    assert!(
        matches!(result, Err(BlockError::Device)),
        "AHCI block device must accept write attempts and return Device error if uninitialized"
    );
}

// ============================================================================
// AHCI driver integration tests
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
