//! ATA driver integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::drivers::ata;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: ATA read rejects out-of-range 28-bit LBA.
/// Given: ATA subsystem was initialized before issuing the call.
/// When: read_sectors is called with LBA > 0x0FFF_FFFF.
/// Then: The function must return AtaError::LbaOutOfRange.
#[test_case]
fn test_ata_read_rejects_lba_out_of_range() {
    ata::init();

    let mut buffer = [0u8; 512];
    let result = ata::read_sectors(&mut buffer, 0x1000_0000, 1);

    assert!(
        matches!(result, Err(ata::AtaError::LbaOutOfRange)),
        "read_sectors must reject LBA values outside 28-bit addressing"
    );
}

/// Contract: ATA write rejects out-of-range 28-bit LBA.
/// Given: ATA subsystem was initialized before issuing the call.
/// When: write_sectors is called with LBA > 0x0FFF_FFFF.
/// Then: The function must return AtaError::LbaOutOfRange.
#[test_case]
fn test_ata_write_rejects_lba_out_of_range() {
    ata::init();

    let buffer = [0xA5u8; 512];
    let result = ata::write_sectors(&buffer, 0x1000_0000, 1);

    assert!(
        matches!(result, Err(ata::AtaError::LbaOutOfRange)),
        "write_sectors must reject LBA values outside 28-bit addressing"
    );
}

/// Contract: ATA init is idempotent.
/// Given: ATA subsystem may already be initialized.
/// When: init is called multiple times.
/// Then: The driver remains usable and still enforces contracts.
#[test_case]
fn test_ata_init_is_idempotent() {
    ata::init();
    ata::init();

    let mut buffer = [0u8; 512];
    let result = ata::read_sectors(&mut buffer, 0x1000_0000, 1);

    assert!(
        matches!(result, Err(ata::AtaError::LbaOutOfRange)),
        "driver must stay operational after repeated init"
    );
}

/// Contract: ATA read with sector_count == 0 is a no-op and returns Ok.
/// Given: ATA subsystem was initialized.
/// When: read_sectors is called with sector_count == 0.
/// Then: The function returns Ok without programming the controller for 256 sectors.
#[test_case]
fn test_ata_read_zero_sectors_is_no_op() {
    ata::init();

    let mut buffer = [0u8; 512];
    let result = ata::read_sectors(&mut buffer, 0, 0);

    assert!(
        result.is_ok(),
        "read_sectors with sector_count == 0 must return Ok without touching hardware"
    );
}

/// Contract: ATA write with sector_count == 0 is a no-op and returns Ok.
/// Given: ATA subsystem was initialized.
/// When: write_sectors is called with sector_count == 0.
/// Then: The function returns Ok without programming the controller for 256 sectors.
#[test_case]
fn test_ata_write_zero_sectors_is_no_op() {
    ata::init();

    let buffer = [0xA5u8; 512];
    let result = ata::write_sectors(&buffer, 0, 0);

    assert!(
        result.is_ok(),
        "write_sectors with sector_count == 0 must return Ok without touching hardware"
    );
}

/// Contract: ATA timeout error variant remains distinct in the public API.
/// Given: The ATA error enum exposes individual failure causes.
/// When: Timeout is compared against other ATA error variants.
/// Then: Timeout must remain distinguishable for callers that handle hangs separately.
#[test_case]
fn test_ata_timeout_error_variant_is_distinct() {
    assert!(
        ata::AtaError::Timeout != ata::AtaError::DeviceError,
        "Timeout must remain distinct from device-reported ERR state"
    );
    assert!(
        ata::AtaError::Timeout != ata::AtaError::DeviceFault,
        "Timeout must remain distinct from device fault (DF) state"
    );
    assert!(
        ata::AtaError::Timeout != ata::AtaError::LbaOutOfRange,
        "Timeout must remain distinct from caller-side input validation errors"
    );
}

/// Contract: ATA write/read roundtrip returns previously written bytes.
/// Given: ATA subsystem was initialized and a writable test sector is chosen.
/// When: A sector is written and read back from the same LBA.
/// Then: The read-back bytes must exactly match the written payload.
#[test_case]
fn test_ata_write_read_roundtrip_returns_written_data() {
    ata::init();

    const TEST_LBA: u32 = 2048;

    let mut original_sector = [0u8; 512];
    let mut read_back = [0u8; 512];
    let mut pattern = [0u8; 512];

    for (idx, byte) in pattern.iter_mut().enumerate() {
        *byte = (idx as u8).wrapping_mul(37).wrapping_add(11);
    }

    let backup_result = ata::read_sectors(&mut original_sector, TEST_LBA, 1);
    assert!(
        backup_result.is_ok(),
        "precondition failed: backup read must succeed before roundtrip write"
    );

    let write_result = ata::write_sectors(&pattern, TEST_LBA, 1);
    assert!(
        write_result.is_ok(),
        "roundtrip write must succeed for test sector"
    );

    let read_result = ata::read_sectors(&mut read_back, TEST_LBA, 1);
    assert!(
        read_result.is_ok(),
        "roundtrip read must succeed for test sector"
    );

    let roundtrip_matches = read_back == pattern;

    let restore_result = ata::write_sectors(&original_sector, TEST_LBA, 1);
    assert!(
        restore_result.is_ok(),
        "test cleanup failed: original sector data must be restorable"
    );

    assert!(
        roundtrip_matches,
        "ATA roundtrip mismatch: read-back bytes differ from written payload"
    );
}

/// Contract: write_sectors performs its post-write completion wait and
/// CACHE FLUSH without erroring or hanging, and the flushed data survives
/// a subsequent read.
///
/// Given: ATA subsystem was initialized and a writable test sector is chosen.
/// When: A multi-sector payload is written (exercising `wait_completion_or_error`
///   and `issue_cache_flush` in `write_sectors` after the transfer loop) and then
///   read back.
/// Then: `write_sectors` must return `Ok`, and the read-back bytes must exactly
///   match what was written, i.e. the flush path neither corrupts data nor
///   returns before the drive has actually accepted the write.
///
/// Note: QEMU's virtual ATA controller completes CACHE FLUSH essentially
/// instantly and does not reproduce the exact real-hardware timing race
/// described in the issue (a fast real CPU sampling stale status right after
/// the command-register write). This test can only prove the new code path
/// is exercised, returns `Ok`, and preserves data end-to-end - it cannot by
/// itself prove the 400 ns settle fixes a real-hardware race.
#[test_case]
fn test_ata_write_sectors_multi_sector_flush_path_preserves_data() {
    ata::init();

    const TEST_LBA: u32 = 2048;
    const SECTOR_COUNT: u8 = 4;
    const TOTAL_BYTES: usize = SECTOR_COUNT as usize * 512;

    let mut original = [0u8; TOTAL_BYTES];
    let mut read_back = [0u8; TOTAL_BYTES];
    let mut pattern = [0u8; TOTAL_BYTES];

    for (idx, byte) in pattern.iter_mut().enumerate() {
        *byte = (idx as u8).wrapping_mul(83).wrapping_add(5);
    }

    let backup_result = ata::read_sectors(&mut original, TEST_LBA, SECTOR_COUNT);
    assert!(
        backup_result.is_ok(),
        "precondition failed: backup read must succeed before flush-path write"
    );

    // This write exercises the post-write BSY-clear/error check and CACHE
    // FLUSH added to `write_sectors`; it must return `Ok` rather than hang
    // or error out on QEMU's virtual controller.
    let write_result = ata::write_sectors(&pattern, TEST_LBA, SECTOR_COUNT);
    assert!(
        write_result.is_ok(),
        "write_sectors must complete its post-write settle/flush and return Ok"
    );

    let read_result = ata::read_sectors(&mut read_back, TEST_LBA, SECTOR_COUNT);
    assert!(read_result.is_ok(), "read after flushed write must succeed");

    let matches = read_back == pattern;

    let restore_result = ata::write_sectors(&original, TEST_LBA, SECTOR_COUNT);
    assert!(
        restore_result.is_ok(),
        "test cleanup failed: original sectors must be restorable"
    );

    assert!(
        matches,
        "data read back after the flush path must match what was written"
    );
}

/// Contract: repeated back-to-back write_sectors calls all succeed.
///
/// Given: ATA subsystem was initialized and a writable test sector is chosen.
/// When: `write_sectors` is called several times in a row to the same LBA,
///   each one running the new post-write completion-wait and CACHE FLUSH
///   logic immediately after the previous call's flush completed.
/// Then: Every call must return `Ok`, and the final write's data must be
///   readable back correctly - i.e. one call's completion/flush wait does
///   not leave the controller in a state that confuses the next command's
///   400 ns settle/setup_command handshake.
///
/// Note: as above, this stresses the code path on QEMU's virtual
/// controller; it demonstrates the new completion-wait/flush logic does
/// not deadlock or corrupt state across repeated commands, but QEMU's
/// timing does not reproduce the real-hardware race the issue describes.
#[test_case]
fn test_ata_write_sectors_back_to_back_calls_all_succeed() {
    ata::init();

    const TEST_LBA: u32 = 2048;
    const ITERATIONS: usize = 5;

    let mut original = [0u8; 512];
    let backup_result = ata::read_sectors(&mut original, TEST_LBA, 1);
    assert!(
        backup_result.is_ok(),
        "precondition failed: backup read must succeed before repeated writes"
    );

    let mut last_pattern = [0u8; 512];

    for iteration in 0..ITERATIONS {
        let mut pattern = [0u8; 512];
        for (idx, byte) in pattern.iter_mut().enumerate() {
            *byte = (idx as u8)
                .wrapping_mul(13)
                .wrapping_add(iteration as u8 * 7);
        }

        let write_result = ata::write_sectors(&pattern, TEST_LBA, 1);
        assert!(
            write_result.is_ok(),
            "write_sectors iteration {} must complete its settle/flush and return Ok",
            iteration
        );

        last_pattern = pattern;
    }

    let mut read_back = [0u8; 512];
    let read_result = ata::read_sectors(&mut read_back, TEST_LBA, 1);
    assert!(
        read_result.is_ok(),
        "read after repeated back-to-back writes must succeed"
    );

    let matches = read_back == last_pattern;

    let restore_result = ata::write_sectors(&original, TEST_LBA, 1);
    assert!(
        restore_result.is_ok(),
        "test cleanup failed: original sector must be restorable"
    );

    assert!(
        matches,
        "data after repeated back-to-back writes must match the last pattern written"
    );
}
