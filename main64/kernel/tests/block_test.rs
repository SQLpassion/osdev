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

/// Contract: AHCI block device LBA validation logic (issue #50).
/// Given: AHCI block device is selected as active.
/// When: read_sectors is called with an LBA beyond AHCI's 48-bit FIS limit
///   (0xFFFF_FFFF_FFFF), e.g. as could result from an unclamped, corrupted
///   GPT `StartingLBA` read as a raw `u64`.
/// Then: The function must return BlockError::OutOfRange *before* any
///   hardware/driver call is attempted (chunked()'s range check runs first),
///   rather than silently truncating the LBA to 48 bits inside the FIS.
#[test_case]
fn test_ahci_block_lba_bounds_checking() {
    // Step 1: Initialize AHCI device registration in block facade.
    block::init_ahci();

    // Step 2: Attempt to read one sector at the first LBA that overflows
    // AHCI's 48-bit addressable range.
    let mut buf = [0u8; 512];
    let result = block::read_sectors(0x1_0000_0000_0000, 1, &mut buf);
    assert!(
        matches!(result, Err(BlockError::OutOfRange)),
        "read_sectors must reject LBA exceeding AHCI's 48-bit FIS limit"
    );

    // Step 3: Same contract must hold for writes.
    let write_result = block::write_sectors(0x1_0000_0000_0000, 1, &buf);
    assert!(
        matches!(write_result, Err(BlockError::OutOfRange)),
        "write_sectors must reject LBA exceeding AHCI's 48-bit FIS limit"
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

/// Contract: AHCI read_sectors rejects sector_count == 0 without touching hardware.
/// Given: No AHCI port needs to be initialized, since the zero-count guard in
///   `read_sectors` must run before any hardware is touched (issue #61, L8).
/// When: `read_sectors` is called with `sector_count == 0`.
/// Then: The call returns `AhciError::InvalidSectorCount`, not `NotInitialized`
///   or a panic — proving the guard is self-contained and runs first.
/// Failure Impact: A future caller that does not pre-validate sector_count (unlike
///   `block.rs`'s current chunking helper) could mis-program the controller, since
///   AHCI interprets a zero sector count as a request for the maximum transfer size.
#[test_case]
fn test_ahci_read_sectors_rejects_zero_sector_count() {
    let mut buf = [0u8; 512];
    let result = ahci::read_sectors(&mut buf, 0, 0);

    assert!(
        matches!(result, Err(ahci::AhciError::InvalidSectorCount)),
        "read_sectors must reject sector_count == 0 with InvalidSectorCount"
    );
}

/// Contract: AHCI write_sectors rejects sector_count == 0 without touching hardware.
/// Given/When/Then: mirrors `test_ahci_read_sectors_rejects_zero_sector_count` for
///   the write path.
#[test_case]
fn test_ahci_write_sectors_rejects_zero_sector_count() {
    let buf = [0xA5u8; 512];
    let result = ahci::write_sectors(&buf, 0, 0);

    assert!(
        matches!(result, Err(ahci::AhciError::InvalidSectorCount)),
        "write_sectors must reject sector_count == 0 with InvalidSectorCount"
    );
}

/// Contract: `max_prdt_entries_for` matches the fixed PRDT capacity boundary.
/// Given: The command table's PRDT holds `AhciError`-adjacent constant
///   `ahci::MAX_PRDT_ENTRIES` (44) entries.
/// When: Computing the worst-case entry count for transfer sizes at, just
///   below, and just above the largest transfer that still fits.
/// Then: The helper's result crosses the `MAX_PRDT_ENTRIES` boundary exactly
///   where a real transfer of that size would overflow the PRDT — this is the
///   pure, hardware-independent seam that stands in for `do_transfer`'s
///   PRDT-overflow check, which cannot be driven end-to-end without a mapped
///   AHCI port (not present in every test environment).
/// Failure Impact: An off-by-one here would either reject legitimate transfers
///   or let an oversized transfer reach the formerly-panicking assert path.
#[test_case]
fn test_ahci_max_prdt_entries_for_boundary() {
    assert_eq!(
        ahci::max_prdt_entries_for(0),
        0,
        "an empty transfer needs no PRDT entries"
    );

    // Largest transfer that still fits in MAX_PRDT_ENTRIES worst-case entries:
    // 1 byte in the first (unaligned) page, then (MAX_PRDT_ENTRIES - 1) full
    // 4 KiB pages.
    let max_fitting_bytes = 1 + (ahci::MAX_PRDT_ENTRIES - 1) * 4096;
    assert_eq!(
        ahci::max_prdt_entries_for(max_fitting_bytes),
        ahci::MAX_PRDT_ENTRIES,
        "largest fitting transfer must need exactly MAX_PRDT_ENTRIES entries"
    );

    // One byte more must require one additional PRDT entry, overflowing the
    // fixed-size table.
    assert_eq!(
        ahci::max_prdt_entries_for(max_fitting_bytes + 1),
        ahci::MAX_PRDT_ENTRIES + 1,
        "a transfer one byte larger must overflow MAX_PRDT_ENTRIES"
    );
}

/// Contract: a transfer whose worst-case PRDT need exceeds capacity is rejected
/// with `AhciError::PrdtOverflow` rather than being sent to `do_transfer`'s
/// panicking assert path.
/// Given: `MAX_PRDT_ENTRIES` is the fixed PRDT capacity and
///   `max_prdt_entries_for` computes the worst-case entries needed.
/// When: A byte count that needs `MAX_PRDT_ENTRIES + 1` entries is evaluated
///   against the capacity, mirroring the guard added to `do_transfer`.
/// Then: The guard condition used in `do_transfer` (`max_prdt_entries_for(n) >
///   MAX_PRDT_ENTRIES`) evaluates to `true`, i.e. the request would be rejected.
#[test_case]
fn test_ahci_prdt_overflow_guard_condition_trips_for_oversized_transfer() {
    let oversized_bytes = 1 + ahci::MAX_PRDT_ENTRIES * 4096;

    assert!(
        ahci::max_prdt_entries_for(oversized_bytes) > ahci::MAX_PRDT_ENTRIES,
        "guard condition must trip for a transfer that cannot fit in the PRDT"
    );
}

/// Contract (issue #48): `do_transfer`'s request-slot primitive enforces
/// "only one AHCI transfer in flight at a time".
///
/// Given: Before issue #48, `AHCI_LOCK` was held (and interrupts disabled)
///   across the *entire* `do_transfer` call, including both busy-poll waits.
///   On this single-core kernel that incidentally also prevented any other
///   task from ever reaching `do_transfer` concurrently, since disabling
///   interrupts disables preemption. Issue #48 narrows `AHCI_LOCK` to only
///   the brief register snapshots/writes, which on its own would reopen a
///   window for two tasks to race on reprogramming command slot 0 (the only
///   slot this driver uses). `do_transfer`'s new `_request` guard
///   (`acquire_transfer_slot`/`AHCI_REQUEST_IN_FLIGHT`) is what closes that
///   window instead.
/// When: The request slot is claimed via the test-only
///   `try_acquire_transfer_slot_for_test` seam (exposed because, like
///   `max_prdt_entries_for` above, `do_transfer`'s hardware path cannot be
///   driven end-to-end without a mapped AHCI port, which is not present in
///   this test environment), and a second claim is attempted while the
///   first is still outstanding.
/// Then: The first claim must succeed, the second must fail while the first
///   is outstanding, and a claim after releasing the first must succeed
///   again.
/// Failure Impact: If this mutual-exclusion primitive were missing or
///   broken, two tasks could concurrently reprogram AHCI command slot 0
///   mid-transfer once `AHCI_LOCK` no longer disables interrupts across the
///   whole call, corrupting in-flight disk I/O.
#[test_case]
fn test_ahci_request_slot_enforces_mutual_exclusion() {
    // First claim of a free slot must succeed.
    let guard1 = ahci::try_acquire_transfer_slot_for_test()
        .expect("first claim of a free request slot must succeed");

    // A second claim while the first is outstanding must fail - this is
    // exactly the invariant `do_transfer`'s `_request` guard relies on.
    assert!(
        ahci::try_acquire_transfer_slot_for_test().is_none(),
        "a second claim must not succeed while the first is outstanding"
    );

    // Releasing the first claim must make the slot claimable again.
    core::mem::drop(guard1);
    let guard2 = ahci::try_acquire_transfer_slot_for_test()
        .expect("the slot must become claimable again after being released");

    // Clean up so later tests in this binary see a free slot.
    core::mem::drop(guard2);
}

/// Contract (issue #48): the request-slot guard never leaks across repeated
/// `do_transfer` calls, even on the early-return "AHCI not initialized"
/// path exercised by this test environment (no `-device ahci` in the QEMU
/// test harness).
///
/// Given: AHCI is not initialized (default QEMU test environment), so every
///   `read_sectors` call returns via `do_transfer`'s early
///   `AhciError::NotInitialized` return, which happens *after* `_request`
///   (the request-slot guard) is constructed.
/// When: `read_sectors` is called many times back-to-back.
/// Then: Every call must return promptly - if the request-slot guard ever
///   failed to run on an early-return path, `AHCI_REQUEST_IN_FLIGHT` would
///   stay stuck `true` and every subsequent call would hang forever waiting
///   for a slot that will never be released (there is no scheduler running
///   in this test binary to preempt out of the fallback spin-wait).
/// Failure Impact: A regression here would deadlock every task doing AHCI
///   I/O after the first call, rather than failing a single call cleanly.
#[test_case]
fn test_ahci_request_slot_does_not_leak_across_repeated_calls() {
    let mut buf = [0u8; 512];

    for lba in 0..200u64 {
        let result = ahci::read_sectors(&mut buf, lba, 1);

        // In this test environment AHCI is never initialized, so every call
        // is expected to fail fast with NotInitialized; tolerate `Ok` too in
        // case a future test environment does provide a mapped AHCI port
        // (mirrors `test_ahci_concurrent_readers_and_multi_sector` above).
        assert!(
            matches!(result, Err(ahci::AhciError::NotInitialized)) || result.is_ok(),
            "unexpected AHCI error on iteration {}: {:?}",
            lba,
            result
        );
    }
}
