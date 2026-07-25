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

// ============================================================================
// find_esp_start_lba Integration Tests
// ============================================================================

use kaos_kernel::drivers::block::{self, BlockDevice, BlockError};
use kaos_kernel::sync::spinlock::SpinLock;

struct DynamicMockBlockDevice {
    fail_lba1: SpinLock<bool>,
    fail_entry: SpinLock<bool>,
    header_sector: SpinLock<[u8; 512]>,
    entry_sector: SpinLock<[u8; 512]>,
}

impl DynamicMockBlockDevice {
    const fn new() -> Self {
        Self {
            fail_lba1: SpinLock::new(false),
            fail_entry: SpinLock::new(false),
            header_sector: SpinLock::new([0u8; 512]),
            entry_sector: SpinLock::new([0u8; 512]),
        }
    }

    fn reset(&self) {
        *self.fail_lba1.lock() = false;
        *self.fail_entry.lock() = false;
        *self.header_sector.lock() = [0u8; 512];
        *self.entry_sector.lock() = [0u8; 512];
    }

    fn setup_valid_header(&self, entry_lba: u64, num_entries: u32, entry_size: u32) {
        let mut hdr = [0u8; 512];
        hdr[0..8].copy_from_slice(b"EFI PART");
        hdr[0x48..0x50].copy_from_slice(&entry_lba.to_le_bytes());
        hdr[0x50..0x54].copy_from_slice(&num_entries.to_le_bytes());
        hdr[0x54..0x58].copy_from_slice(&entry_size.to_le_bytes());
        *self.header_sector.lock() = hdr;
    }
}

impl BlockDevice for DynamicMockBlockDevice {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        if count != 1 || buf.len() < 512 {
            return Err(BlockError::BadBuffer);
        }
        if lba == 1 {
            if *self.fail_lba1.lock() {
                return Err(BlockError::Device);
            }
            buf[..512].copy_from_slice(&*self.header_sector.lock());
            Ok(())
        } else {
            if *self.fail_entry.lock() {
                return Err(BlockError::Device);
            }
            buf[..512].copy_from_slice(&*self.entry_sector.lock());
            Ok(())
        }
    }

    fn write_sectors(&self, _lba: u64, _count: u32, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::Unsupported)
    }
}

static TEST_MOCK_DEV: DynamicMockBlockDevice = DynamicMockBlockDevice::new();

#[test_case]
fn test_find_esp_start_lba_unreadable_header() {
    TEST_MOCK_DEV.reset();
    *TEST_MOCK_DEV.fail_lba1.lock() = true;
    block::set_active_device(&TEST_MOCK_DEV);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_invalid_signature() {
    TEST_MOCK_DEV.reset();
    // Header is zeroed (invalid signature)
    block::set_active_device(&TEST_MOCK_DEV);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_unreadable_entry_sector() {
    TEST_MOCK_DEV.reset();
    TEST_MOCK_DEV.setup_valid_header(2, 128, 128);
    *TEST_MOCK_DEV.fail_entry.lock() = true;
    block::set_active_device(&TEST_MOCK_DEV);

    assert_eq!(gpt::find_esp_start_lba(), None);

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_valid_gpt_no_esp_entry() {
    TEST_MOCK_DEV.reset();
    TEST_MOCK_DEV.setup_valid_header(2, 128, 128);
    // Entry sector is zeroed (no matching ESP GUID)
    block::set_active_device(&TEST_MOCK_DEV);

    assert_eq!(gpt::find_esp_start_lba(), Some(2048));

    block::reset_active_device();
}

#[test_case]
fn test_find_esp_start_lba_valid_gpt_with_esp() {
    TEST_MOCK_DEV.reset();
    TEST_MOCK_DEV.setup_valid_header(2, 128, 128);

    let mut entry_sec = [0u8; 512];
    entry_sec[0..16].copy_from_slice(&ESP_TYPE_GUID);
    entry_sec[0x20..0x28].copy_from_slice(&4096u64.to_le_bytes());
    *TEST_MOCK_DEV.entry_sector.lock() = entry_sec;

    block::set_active_device(&TEST_MOCK_DEV);

    assert_eq!(gpt::find_esp_start_lba(), Some(4096));

    block::reset_active_device();
}
