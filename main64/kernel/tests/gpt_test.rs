//! GPT parsing integration tests
//!
//! Verifies the pure parsing functions of the GPT implementation.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::io::gpt;

/// Entry point for the integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

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

const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

#[test_case]
fn test_parse_gpt_header_valid() {
    let mut header = [0u8; 512];
    header[0..8].copy_from_slice(b"EFI PART");

    // PartitionEntryLBA = 2
    header[0x48..0x50].copy_from_slice(&2u64.to_le_bytes());
    // NumberOfPartitionEntries = 128
    header[0x50..0x54].copy_from_slice(&128u32.to_le_bytes());
    // SizeOfPartitionEntry = 128
    header[0x54..0x58].copy_from_slice(&128u32.to_le_bytes());

    let result = gpt::parse_gpt_header(&header);
    assert_eq!(result, Some((2, 128, 128)));
}

#[test_case]
fn test_parse_gpt_header_invalid_signature() {
    let mut header = [0u8; 512];
    header[0..8].copy_from_slice(b"BAD PART");
    assert_eq!(gpt::parse_gpt_header(&header), None);
}

#[test_case]
fn test_parse_gpt_header_invalid_entry_size() {
    let mut header = [0u8; 512];
    header[0..8].copy_from_slice(b"EFI PART");
    header[0x48..0x50].copy_from_slice(&2u64.to_le_bytes());
    header[0x50..0x54].copy_from_slice(&128u32.to_le_bytes());
    // SizeOfPartitionEntry = 0
    header[0x54..0x58].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(gpt::parse_gpt_header(&header), None);

    // SizeOfPartitionEntry = 123 (not cleanly dividing 512)
    header[0x54..0x58].copy_from_slice(&123u32.to_le_bytes());
    assert_eq!(gpt::parse_gpt_header(&header), None);
}

#[test_case]
fn test_parse_gpt_header_rejects_small_divisors_of_512() {
    // Regression test for H1 / issue #42: entry sizes that cleanly divide 512
    // but are below the UEFI-mandated minimum of 128 bytes must be rejected
    // by the header validation, not accepted and later fed to
    // `parse_gpt_entries_sector` where they cause an out-of-bounds slice.
    let mut header = [0u8; 512];
    header[0..8].copy_from_slice(b"EFI PART");
    header[0x48..0x50].copy_from_slice(&2u64.to_le_bytes());
    header[0x50..0x54].copy_from_slice(&128u32.to_le_bytes());

    for &entry_size in &[8u32, 32, 64] {
        header[0x54..0x58].copy_from_slice(&entry_size.to_le_bytes());
        assert_eq!(gpt::parse_gpt_header(&header), None);
    }
}

#[test_case]
fn test_parse_gpt_header_accepts_minimum_valid_entry_size() {
    // SizeOfPartitionEntry = 128 is the UEFI-mandated minimum and must still
    // be accepted.
    let mut header = [0u8; 512];
    header[0..8].copy_from_slice(b"EFI PART");
    header[0x48..0x50].copy_from_slice(&2u64.to_le_bytes());
    header[0x50..0x54].copy_from_slice(&128u32.to_le_bytes());
    header[0x54..0x58].copy_from_slice(&128u32.to_le_bytes());

    assert_eq!(gpt::parse_gpt_header(&header), Some((2, 128, 128)));
}

#[test_case]
fn test_parse_gpt_entries_sector_found() {
    let mut sector = [0u8; 512];
    let entry_size = 128;

    // Second entry (offset 128) is ESP
    sector[128..128 + 16].copy_from_slice(&ESP_TYPE_GUID);
    // Start LBA = 2048
    sector[128 + 0x20..128 + 0x28].copy_from_slice(&2048u64.to_le_bytes());

    let result = gpt::parse_gpt_entries_sector(&sector, 4, entry_size);
    assert_eq!(result, Some(2048));
}

#[test_case]
fn test_parse_gpt_entries_sector_not_found() {
    let mut sector = [0u8; 512];
    let entry_size = 128;

    // Use a dummy GUID for all entries
    let dummy_guid = [0x11; 16];
    sector[0..16].copy_from_slice(&dummy_guid);
    sector[128..128 + 16].copy_from_slice(&dummy_guid);

    let result = gpt::parse_gpt_entries_sector(&sector, 4, entry_size);
    assert_eq!(result, None);
}

#[test_case]
fn test_parse_gpt_entries_sector_small_entry_size_does_not_panic() {
    // Regression test for H1 / issue #42: this is the actual OOB-slice bug -
    // `parse_gpt_header` now rejects `entry_size = 8` before it ever reaches
    // this function, but `parse_gpt_entries_sector` itself must still not
    // panic if invoked directly with a small `entry_size` (e.g. by a future
    // caller that bypasses header validation). With `entry_size = 8` and
    // `entries_in_this_sector = 64`, the last iteration (`i = 63`) computes
    // `offset = 504`, and `504 + 0x28 = 544 > 512` used to panic on the
    // unchecked slice; it must now gracefully break and return `None`.
    let entry_size = 8;
    let sector = [0u8; 512];

    let result = gpt::parse_gpt_entries_sector(&sector, 64, entry_size);
    assert_eq!(result, None);
}

// ============================================================================
// find_esp_start_lba() integration tests
//
// These exercise the full function (including its I/O calls) by injecting a
// mock `BlockDevice` via `block::set_active_device`, so every failure branch
// documented in issue #17 can be driven deterministically without real disk
// hardware. See docs/testing.md for how `#[test_case]` tests are structured.
// ============================================================================

use kaos_kernel::drivers::block::{self, BlockDevice, BlockError};
use kaos_kernel::sync::spinlock::SpinLock;

/// A `BlockDevice` whose behavior per-LBA is configured at test setup time, so
/// each test can simulate a specific GPT on-disk layout or I/O failure.
struct MockBlockDevice {
    /// Sector returned for reads of LBA 1 (the GPT header). `None` means "fail".
    lba1_sector: SpinLock<Option<[u8; 512]>>,
    /// Sector returned for reads of any other LBA (partition entries). `None` means "fail".
    entry_sector: SpinLock<Option<[u8; 512]>>,
}

impl MockBlockDevice {
    const fn new() -> Self {
        Self {
            lba1_sector: SpinLock::new(None),
            entry_sector: SpinLock::new(None),
        }
    }

    /// Configure a valid GPT header for LBA 1 with the given partition-array metadata.
    fn set_valid_header(&self, entry_lba: u64, num_entries: u32, entry_size: u32) {
        let mut hdr = [0u8; 512];
        hdr[0..8].copy_from_slice(b"EFI PART");
        hdr[0x48..0x50].copy_from_slice(&entry_lba.to_le_bytes());
        hdr[0x50..0x54].copy_from_slice(&num_entries.to_le_bytes());
        hdr[0x54..0x58].copy_from_slice(&entry_size.to_le_bytes());
        *self.lba1_sector.lock() = Some(hdr);
    }

    /// Configure the entry sector to contain a single ESP entry at `start_lba`.
    fn set_esp_entry(&self, start_lba: u64) {
        let mut sector = [0u8; 512];
        sector[0..16].copy_from_slice(&ESP_TYPE_GUID);
        sector[0x20..0x28].copy_from_slice(&start_lba.to_le_bytes());
        *self.entry_sector.lock() = Some(sector);
    }

    /// Configure the entry sector as present but containing no ESP entry.
    fn set_entry_sector_no_esp(&self) {
        *self.entry_sector.lock() = Some([0u8; 512]);
    }

    fn fail_lba1(&self) {
        *self.lba1_sector.lock() = None;
    }

    fn fail_entry_sector(&self) {
        *self.entry_sector.lock() = None;
    }
}

impl BlockDevice for MockBlockDevice {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        if count != 1 || buf.len() < 512 {
            return Err(BlockError::BadBuffer);
        }

        let slot = if lba == 1 {
            &self.lba1_sector
        } else {
            &self.entry_sector
        };

        match *slot.lock() {
            Some(sector) => {
                buf[..512].copy_from_slice(&sector);
                Ok(())
            }
            None => Err(BlockError::Device),
        }
    }

    fn write_sectors(&self, _lba: u64, _count: u32, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::Unsupported)
    }
}

static MOCK_DEVICE: MockBlockDevice = MockBlockDevice::new();

#[test_case]
fn test_find_esp_start_lba_header_read_failure_returns_none() {
    // LBA 1 (the GPT header sector) is unreadable, e.g. no disk / a hardware error.
    MOCK_DEVICE.fail_lba1();
    MOCK_DEVICE.fail_entry_sector();
    block::set_active_device(&MOCK_DEVICE);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_invalid_signature_returns_none() {
    // LBA 1 reads fine but does not contain a valid "EFI PART" signature -
    // there is simply no GPT on this disk.
    let mut bad_header = [0u8; 512];
    bad_header[0..8].copy_from_slice(b"BAD SIG!");
    *MOCK_DEVICE.lba1_sector.lock() = Some(bad_header);
    MOCK_DEVICE.fail_entry_sector();
    block::set_active_device(&MOCK_DEVICE);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_entry_sector_read_failure_returns_none() {
    // The header parses correctly, but the partition-entry array itself
    // cannot be read (e.g. bad sector further into the disk).
    MOCK_DEVICE.set_valid_header(2, 128, 128);
    MOCK_DEVICE.fail_entry_sector();
    block::set_active_device(&MOCK_DEVICE);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_valid_gpt_no_esp_falls_back_to_2048() {
    // A fully valid, readable GPT - but none of its entries is an ESP. This is
    // the one case that should still legitimately fall back to LBA 2048.
    MOCK_DEVICE.set_valid_header(2, 128, 128);
    MOCK_DEVICE.set_entry_sector_no_esp();
    block::set_active_device(&MOCK_DEVICE);

    assert_eq!(gpt::find_esp_start_lba(), Some(2048));

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_valid_gpt_with_esp_returns_its_lba() {
    // A fully valid GPT whose partition array declares an ESP - the real LBA
    // from the entry must be returned, not the 2048 heuristic.
    MOCK_DEVICE.set_valid_header(2, 128, 128);
    MOCK_DEVICE.set_esp_entry(4096);
    block::set_active_device(&MOCK_DEVICE);

    assert_eq!(gpt::find_esp_start_lba(), Some(4096));

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_small_entry_size_returns_none_not_panic() {
    // Regression test for H1 / issue #42: a disk with a valid "EFI PART"
    // signature, NumberOfPartitionEntries = 128, but SizeOfPartitionEntry = 8
    // (a divisor of 512, but below the UEFI-mandated minimum of 128) used to
    // panic the kernel via an out-of-bounds slice deep inside
    // `parse_gpt_entries_sector`. It must now be rejected gracefully at the
    // header-validation stage and reported as `None`, not panic and not fall
    // back to the LBA 2048 heuristic (which is reserved for "valid GPT, no
    // ESP entry" only).
    MOCK_DEVICE.set_valid_header(2, 128, 8);
    MOCK_DEVICE.set_esp_entry(4096);
    block::set_active_device(&MOCK_DEVICE);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}
