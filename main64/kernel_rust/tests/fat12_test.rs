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
use kaos_kernel::process;

const ROOT_DIR_BYTES: usize = 224 * 32;
const ENTRY_SIZE: usize = 32;
const BYTES_PER_SECTOR: usize = 512;
const FAT1_LBA: u32 = 1;
const FAT_SECTORS: u8 = 9;
const ROOT_DIRECTORY_LBA: u32 = 19;
const ROOT_DIRECTORY_SECTORS: u8 = 14;
const EXPECTED_SFILE_BYTES: &[u8] = include_bytes!("../../SFile.txt");
const EXPECTED_HELLO_BIN_BYTES: &[u8] = include_bytes!("../../user_programs/hello/hello.bin");
const EXPECTED_READLINE_BIN_BYTES: &[u8] =
    include_bytes!("../../user_programs/readline/readline.bin");

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

fn find_first_cluster_in_root(root: &[u8], short_name: &[u8; 11]) -> Option<u16> {
    for entry_idx in 0..(root.len() / ENTRY_SIZE) {
        let start = entry_idx * ENTRY_SIZE;
        let entry = &root[start..start + ENTRY_SIZE];

        if entry[0] == 0x00 {
            break;
        }
        if entry[0] == 0xE5 {
            continue;
        }
        if entry[11] == 0x0F || (entry[11] & 0x08) != 0 {
            continue;
        }
        if &entry[0..11] != short_name {
            continue;
        }

        return Some(u16::from_le_bytes([entry[26], entry[27]]));
    }

    None
}

fn write_fat12_entry(fat: &mut [u8], cluster: u16, value: u16) {
    let cluster_index = cluster as usize;
    let offset = cluster_index + (cluster_index / 2);
    let value = value & 0x0FFF;

    if cluster & 1 == 0 {
        fat[offset] = (value & 0x00FF) as u8;
        fat[offset + 1] = (fat[offset + 1] & 0xF0) | ((value >> 8) as u8 & 0x0F);
    } else {
        fat[offset] = (fat[offset] & 0x0F) | (((value << 4) as u8) & 0xF0);
        fat[offset + 1] = (value >> 4) as u8;
    }
}

fn read_file_with_patched_next_cluster(file_name: &str, patched_next_cluster: u16) -> Result<(), Fat12Error> {
    // Step 1: Resolve the first cluster of the target file from root directory entries.
    let short_name = normalize_8_3_name(file_name).expect("short name must normalize");
    let mut root = [0u8; ROOT_DIRECTORY_SECTORS as usize * BYTES_PER_SECTOR];
    kaos_kernel::drivers::ata::read_sectors(&mut root, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)
        .expect("root directory must be readable");
    let first_cluster =
        find_first_cluster_in_root(&root, &short_name).expect("directory entry must exist");

    // Step 2: Patch FAT#1 next-pointer for that first cluster.
    let mut original_fat = [0u8; FAT_SECTORS as usize * BYTES_PER_SECTOR];
    kaos_kernel::drivers::ata::read_sectors(&mut original_fat, FAT1_LBA, FAT_SECTORS)
        .expect("FAT#1 must be readable");
    let mut patched_fat = original_fat;
    write_fat12_entry(&mut patched_fat, first_cluster, patched_next_cluster);
    kaos_kernel::drivers::ata::write_sectors(&patched_fat, FAT1_LBA, FAT_SECTORS)
        .expect("FAT#1 patch write must succeed");

    // Step 3: Execute read path while FAT is patched, then restore original FAT bytes.
    let read_result = read_file(file_name);
    kaos_kernel::drivers::ata::write_sectors(&original_fat, FAT1_LBA, FAT_SECTORS)
        .expect("FAT#1 restore must succeed");

    match read_result {
        Ok(_) => Ok(()),
        Err(err) => Err(err),
    }
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

/// Contract: parse_root_directory summary counts only regular files, not directories.
#[test_case]
fn test_parser_summary_excludes_directories_from_file_count() {
    let mut root = [0u8; ROOT_DIR_BYTES];

    write_entry(&mut root, 0, b"SUBDIR  ", b"   ", 0x10, 5, 0);
    write_entry(&mut root, 1, b"README  ", b"TXT", 0x20, 7, 42);
    root[2 * ENTRY_SIZE] = 0x00;

    let mut callback_count = 0usize;
    let (file_count, total_size) = parse_root_directory(&root, |_| {
        callback_count += 1;
    });

    assert!(
        callback_count == 2,
        "callback must still receive both directory and file entries"
    );
    assert!(
        file_count == 1,
        "summary file count must exclude directory entries"
    );
    assert!(
        total_size == 42,
        "summary byte count must include only regular files"
    );
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

/// Contract: parser omits the dot when the 8.3 extension field is empty.
#[test_case]
fn test_parser_formats_short_name_without_extension() {
    let mut root = [0u8; ROOT_DIR_BYTES];
    write_entry(&mut root, 0, b"KERNEL  ", b"   ", 0x20, 9, 100);
    root[ENTRY_SIZE] = 0x00;

    let mut record = None::<RootDirectoryRecord>;
    let _ = parse_root_directory(&root, |entry| {
        record = Some(entry);
    });

    let record = record.expect("expected one parsed entry");
    let formatted =
        core::str::from_utf8(&record.name[..record.name_len]).expect("name bytes must be ASCII");

    assert!(
        formatted == "kernel",
        "name with empty extension must not include a trailing dot"
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

/// Contract: normalize_8_3_name rejects trailing dot tokens (`name.`).
#[test_case]
fn test_normalize_8_3_name_rejects_trailing_dot() {
    let result = normalize_8_3_name("kernel.");
    assert!(
        matches!(result, Err(Fat12Error::InvalidFileName)),
        "trailing dot must be rejected as invalid FAT short name"
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

/// Contract: bundled user program binary can be read from FAT12 image.
#[test_case]
fn test_read_file_hello_bin_present_and_within_user_image_limit() {
    kaos_kernel::drivers::ata::init();

    let actual = read_file("hello.bin").expect("hello.bin must be readable from FAT12 image");
    assert!(
        !actual.is_empty(),
        "hello.bin must contain executable bytes in FAT12 image"
    );
    assert!(
        actual.len() <= process::USER_PROGRAM_MAX_IMAGE_SIZE,
        "hello.bin size must fit configured user executable window"
    );
}

/// Contract: `read_file("hello.bin")` returns exact bytes from FAT12 image.
#[test_case]
fn test_read_file_hello_bin_returns_exact_bytes() {
    kaos_kernel::drivers::ata::init();

    let actual = read_file("hello.bin").expect("hello.bin must be readable from FAT12 image");
    assert!(
        actual.len() == EXPECTED_HELLO_BIN_BYTES.len(),
        "read_file length mismatch: expected {} bytes, got {}",
        EXPECTED_HELLO_BIN_BYTES.len(),
        actual.len()
    );

    for idx in 0..EXPECTED_HELLO_BIN_BYTES.len() {
        assert!(
            actual[idx] == EXPECTED_HELLO_BIN_BYTES[idx],
            "byte mismatch at offset {}: expected 0x{:02x}, got 0x{:02x}",
            idx,
            EXPECTED_HELLO_BIN_BYTES[idx],
            actual[idx]
        );
    }
}

/// Contract: `read_file("readline.bin")` returns exact bytes from FAT12 image.
#[test_case]
fn test_read_file_readline_bin_returns_exact_bytes() {
    kaos_kernel::drivers::ata::init();

    let actual = read_file("readline.bin")
        .expect("readline.bin must be readable from FAT12 image");
    assert!(
        actual.len() == EXPECTED_READLINE_BIN_BYTES.len(),
        "read_file length mismatch: expected {} bytes, got {}",
        EXPECTED_READLINE_BIN_BYTES.len(),
        actual.len()
    );

    for idx in 0..EXPECTED_READLINE_BIN_BYTES.len() {
        assert!(
            actual[idx] == EXPECTED_READLINE_BIN_BYTES[idx],
            "byte mismatch at offset {}: expected 0x{:02x}, got 0x{:02x}",
            idx,
            EXPECTED_READLINE_BIN_BYTES[idx],
            actual[idx]
        );
    }
}

/// Contract: a FAT chain that jumps to cluster 1 is reported as CorruptFatChain.
#[test_case]
fn test_read_file_reports_corrupt_fat_chain_for_cluster_1_target() {
    kaos_kernel::drivers::ata::init();

    let result = read_file_with_patched_next_cluster("hello.bin", 1);

    assert!(
        matches!(result, Err(Fat12Error::CorruptFatChain)),
        "cluster target 1 must be classified as CorruptFatChain, got {:?}",
        result
    );
}

/// Contract: a FAT chain that jumps to cluster 0 is reported as CorruptFatChain.
#[test_case]
fn test_read_file_reports_corrupt_fat_chain_for_cluster_0_target() {
    kaos_kernel::drivers::ata::init();

    let result = read_file_with_patched_next_cluster("hello.bin", 0);

    assert!(
        matches!(result, Err(Fat12Error::CorruptFatChain)),
        "cluster target 0 must be classified as CorruptFatChain, got {:?}",
        result
    );
}
