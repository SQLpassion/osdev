//! FAT12 root directory parser tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::io::fat12::{
    normalize_8_3_name, parse_root_directory, read_file, Fat12Error, RootDirectoryRecord,
};

const ROOT_DIR_BYTES: usize = 224 * 32;
const ENTRY_SIZE: usize = 32;
const EXPECTED_SFILE_BYTES: &[u8] = include_bytes!("../../SFile.txt");

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

fn write_entry(
    root: &mut [u8; ROOT_DIR_BYTES],
    index: usize,
    name: &[u8; 8],
    extension: &[u8; 3],
    attributes: u8,
    first_cluster: u16,
    file_size: u32,
) {
    let start = index * ENTRY_SIZE;
    root[start..start + ENTRY_SIZE].fill(0);
    root[start..start + 8].copy_from_slice(name);
    root[start + 8..start + 11].copy_from_slice(extension);
    root[start + 11] = attributes;
    root[start + 26..start + 28].copy_from_slice(&first_cluster.to_le_bytes());
    root[start + 28..start + 32].copy_from_slice(&file_size.to_le_bytes());
}

/// Contract: parser skips deleted, LFN, and volume-label entries.
#[test_case]
fn test_parser_skips_non_file_entries() {
    let mut root = [0u8; ROOT_DIR_BYTES];

    write_entry(&mut root, 0, b"KERNEL  ", b"BIN", 0x20, 2, 1234);
    write_entry(&mut root, 1, b"DELETED ", b"TXT", 0x20, 3, 100);
    root[ENTRY_SIZE] = 0xE5;
    write_entry(&mut root, 2, b"LFNENT  ", b"TXT", 0x0F, 4, 200);
    write_entry(&mut root, 3, b"VOLUME  ", b"LBL", 0x08, 5, 300);
    root[4 * ENTRY_SIZE] = 0x00;

    let mut parsed = [None; 4];
    let mut parsed_len = 0usize;

    let (file_count, total_size) = parse_root_directory(&root, |entry| {
        if parsed_len < parsed.len() {
            parsed[parsed_len] = Some(entry);
        }
        parsed_len += 1;
    });

    assert!(file_count == 1, "only one file entry must be accepted");
    assert!(total_size == 1234, "total size must include only accepted entries");
    assert!(parsed_len == 1, "callback must run exactly once");

    let first = parsed[0].expect("first parsed entry must exist");
    assert!(
        first.first_cluster == 2,
        "parsed entry must preserve first cluster"
    );
}

/// Contract: parser stops scanning at FAT12 end marker (0x00 first byte).
#[test_case]
fn test_parser_stops_on_end_marker() {
    let mut root = [0u8; ROOT_DIR_BYTES];

    write_entry(&mut root, 0, b"FIRST   ", b"TXT", 0x20, 2, 10);
    root[ENTRY_SIZE] = 0x00;
    write_entry(&mut root, 2, b"SECOND  ", b"TXT", 0x20, 3, 20);

    let mut parsed_len = 0usize;
    let (file_count, total_size) = parse_root_directory(&root, |_| {
        parsed_len += 1;
    });

    assert!(file_count == 1, "entries after end marker must not be parsed");
    assert!(parsed_len == 1, "callback must stop at end marker");
    assert!(total_size == 10, "size sum must stop at end marker");
}

/// Contract: parser formats 8.3 names as lowercase `name.ext`.
#[test_case]
fn test_parser_formats_short_name_lowercase() {
    let mut root = [0u8; ROOT_DIR_BYTES];
    write_entry(&mut root, 0, b"README  ", b"TXT", 0x20, 7, 42);
    root[ENTRY_SIZE] = 0x00;

    let mut record = None::<RootDirectoryRecord>;
    let _ = parse_root_directory(&root, |entry| {
        record = Some(entry);
    });

    let record = record.expect("expected one parsed entry");
    let formatted =
        core::str::from_utf8(&record.name[..record.name_len]).expect("name bytes must be ASCII");

    assert!(
        formatted == "readme.txt",
        "8.3 name must be rendered as lowercase name.ext"
    );
    assert!(record.file_size == 42, "file size must match source entry");
    assert!(
        record.first_cluster == 7,
        "first cluster must match source entry"
    );
}

/// Contract: 8.3 normalization returns uppercased, space-padded FAT short name.
#[test_case]
fn test_normalize_8_3_name_returns_expected_short_name() {
    let normalized = normalize_8_3_name("readme.txt").expect("8.3 name must normalize");
    assert!(
        normalized == *b"README  TXT",
        "normalized FAT short name must be uppercase and space-padded"
    );
}

/// Contract: read_file rejects invalid short-name inputs before touching disk.
#[test_case]
fn test_read_file_rejects_invalid_short_name() {
    let result = read_file("invalid.name.txt");
    assert!(
        matches!(result, Err(Fat12Error::InvalidFileName)),
        "read_file must reject invalid 8.3 names"
    );
}

/// Contract: `read_file("sfile.txt")` returns exactly the on-disk file bytes.
#[test_case]
fn test_read_file_sfile_returns_exact_bytes() {
    kaos_kernel::drivers::ata::init();

    let actual = read_file("sfile.txt").expect("sfile.txt must be readable from FAT12 image");
    assert!(
        actual.len() == EXPECTED_SFILE_BYTES.len(),
        "read_file length mismatch: expected {} bytes, got {}",
        EXPECTED_SFILE_BYTES.len(),
        actual.len()
    );

    for idx in 0..EXPECTED_SFILE_BYTES.len() {
        assert!(
            actual[idx] == EXPECTED_SFILE_BYTES[idx],
            "byte mismatch at offset {}: expected 0x{:02x}, got 0x{:02x}",
            idx,
            EXPECTED_SFILE_BYTES[idx],
            actual[idx]
        );
    }
}

/// Contract: FAT short-name lookup is case-insensitive for user input.
#[test_case]
fn test_read_file_uppercase_name_returns_same_bytes() {
    kaos_kernel::drivers::ata::init();

    let actual = read_file("SFILE.TXT").expect("SFILE.TXT must be readable from FAT12 image");
    assert!(
        actual.len() == EXPECTED_SFILE_BYTES.len(),
        "read_file length mismatch: expected {} bytes, got {}",
        EXPECTED_SFILE_BYTES.len(),
        actual.len()
    );

    for idx in 0..EXPECTED_SFILE_BYTES.len() {
        assert!(
            actual[idx] == EXPECTED_SFILE_BYTES[idx],
            "byte mismatch at offset {}: expected 0x{:02x}, got 0x{:02x}",
            idx,
            EXPECTED_SFILE_BYTES[idx],
            actual[idx]
        );
    }
}
