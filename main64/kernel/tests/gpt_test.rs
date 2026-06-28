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
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
    0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
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
