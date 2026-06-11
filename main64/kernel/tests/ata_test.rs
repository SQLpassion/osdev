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
