//! FAT12 File System Driver
//!
//! Reads the FAT12 root directory from disk and provides functionality
//! to list its entries. The FAT12 layout matches a standard 1.44 MB
//! floppy disk image used by the KAOS bootloader.

use crate::drivers;
use alloc::vec;
use core::fmt::Write;

// FAT12 disk geometry constants for a 1.44 MB floppy layout.
const BYTES_PER_SECTOR: usize = 512;
const FAT_COUNT: u32 = 2;
const SECTORS_PER_FAT: u32 = 9;
const RESERVED_SECTORS: u32 = 1;
const ROOT_DIRECTORY_ENTRIES: usize = 224;

/// LBA address of the root directory: FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS = 19
const ROOT_DIRECTORY_LBA: u32 = FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS;

/// Number of sectors occupied by the root directory: 32 * 224 / 512 = 14
const ROOT_DIRECTORY_SECTORS: u8 = (32 * ROOT_DIRECTORY_ENTRIES / BYTES_PER_SECTOR) as u8;

// FAT12 root directory entry layout (all offsets within one 32-byte entry).
const DIRECTORY_ENTRY_SIZE: usize = 32;
const ATTR_OFFSET: usize = 11;
const FIRST_CLUSTER_OFFSET: usize = 26;
const FILE_SIZE_OFFSET: usize = 28;

// Attribute flags used by entry filtering.
const ATTR_LONG_FILE_NAME: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;

/// Parsed root-directory entry in a format convenient for shell output/tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootDirectoryRecord {
    pub name: [u8; 13],
    pub name_len: usize,
    pub first_cluster: u16,
    pub file_size: u32,
}

#[derive(Clone, Copy)]
enum EntryState {
    /// Entry byte 0 is 0x00: no more entries in this directory table.
    End,

    /// Entry exists in table but should not be exposed by `dir`.
    Skip,

    /// Normal short (8.3) file/directory entry.
    Active,
}

/// Raw 32-byte on-disk FAT12 root directory entry.
#[derive(Clone, Copy)]
struct RawRootDirectoryEntry {
    bytes: [u8; DIRECTORY_ENTRY_SIZE],
}

impl RawRootDirectoryEntry {
    /// Classify this raw entry according to FAT12 root-directory rules.
    fn state(&self) -> EntryState {
        let first_name_byte = self.bytes[0];

        if first_name_byte == 0x00 {
            return EntryState::End;
        }

        if first_name_byte == 0xE5 {
            return EntryState::Skip;
        }

        let attributes = self.bytes[ATTR_OFFSET];

        // Skip LFN helper entries and volume labels from `dir` output.
        if attributes == ATTR_LONG_FILE_NAME || (attributes & ATTR_VOLUME_ID) != 0 {
            return EntryState::Skip;
        }

        EntryState::Active
    }

    /// Decode raw on-disk fields into a high-level record.
    fn parse_record(&self) -> RootDirectoryRecord {
        let mut name = [0u8; 13];
        let mut pos = 0;

        // File name: up to 8 characters, strip trailing spaces
        for &b in &self.bytes[0..8] {
            if b == b' ' {
                break;
            }

            name[pos] = b.to_ascii_lowercase();
            pos += 1;
        }

        // Extension: up to 3 characters, strip trailing spaces
        let extension = &self.bytes[8..11];
        let ext_start = extension.iter().position(|&b| b != b' ');

        // Add `.ext` only when an extension is actually present.
        if ext_start.is_some() {
            name[pos] = b'.';
            pos += 1;

            for &b in extension {
                if b == b' ' {
                    break;
                }

                name[pos] = b.to_ascii_lowercase();
                pos += 1;
            }
        }

        let first_cluster = u16::from_le_bytes([
            self.bytes[FIRST_CLUSTER_OFFSET],
            self.bytes[FIRST_CLUSTER_OFFSET + 1],
        ]);

        let file_size = u32::from_le_bytes([
            self.bytes[FILE_SIZE_OFFSET],
            self.bytes[FILE_SIZE_OFFSET + 1],
            self.bytes[FILE_SIZE_OFFSET + 2],
            self.bytes[FILE_SIZE_OFFSET + 3],
        ]);

        RootDirectoryRecord {
            name,
            name_len: pos,
            first_cluster,
            file_size,
        }
    }
}

/// Initialize the FAT12 file system.
///
/// Must be called after `drivers::ata::init()`.
///
/// The implementation is intentionally cache-free. Root directory data is
/// always read fresh from disk when requested.
pub fn init() {}

/// Read the fixed-size FAT12 root directory area from disk.
///
/// For standard FAT12 this is always 14 sectors at LBA 19.
fn read_root_directory_from_disk() -> alloc::vec::Vec<u8> {
    let mut buffer = vec![0u8; ROOT_DIRECTORY_SECTORS as usize * BYTES_PER_SECTOR];
    drivers::ata::read_sectors(&mut buffer, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)
        .expect("Failed to read FAT12 root directory from disk");

    buffer
}

/// Parse all visible root-directory entries and call `on_entry` for each one.
///
/// Returns `(file_count, total_size)` for printed summary output.
pub fn parse_root_directory<F>(buffer: &[u8], mut on_entry: F) -> (u32, u32)
where
    F: FnMut(RootDirectoryRecord),
{
    let mut file_count: u32 = 0;
    let mut total_size: u32 = 0;

    let entry_count = core::cmp::min(ROOT_DIRECTORY_ENTRIES, buffer.len() / DIRECTORY_ENTRY_SIZE);

    for entry_idx in 0..entry_count {
        let start = entry_idx * DIRECTORY_ENTRY_SIZE;
        let mut bytes = [0u8; DIRECTORY_ENTRY_SIZE];
        bytes.copy_from_slice(&buffer[start..start + DIRECTORY_ENTRY_SIZE]);

        let entry = RawRootDirectoryEntry { bytes };

        match entry.state() {
            EntryState::End => break,
            EntryState::Skip => continue,
            EntryState::Active => {
                let record = entry.parse_record();
                on_entry(record);

                file_count += 1;
                total_size += record.file_size;
            }
        }
    }

    (file_count, total_size)
}

/// Print all active root directory entries to the VGA screen.
///
/// Output format matches the C implementation:
/// ```text
/// <size> bytes    Start Cluster: <cluster>    <name>.<ext>
///         <count> File(s)    <total> bytes
/// ```
pub fn print_root_directory() {
    // Cache-free behavior: always read current directory state from disk.
    let root_dir = read_root_directory_from_disk();

    drivers::screen::with_screen(|screen| {
        let (file_count, total_size) = parse_root_directory(&root_dir, |entry| {
            let name = core::str::from_utf8(&entry.name[..entry.name_len]).unwrap_or("???");

            let _ = write!(screen, "{} bytes", entry.file_size);
            let _ = write!(screen, "\tStart Cluster: {}", entry.first_cluster);
            let _ = write!(screen, "\t{}", name);
            screen.print_char(b'\n');
        });

        let _ = write!(screen, "\t\t{} File(s)", file_count);
        let _ = write!(screen, "\t{} bytes", total_size);
        screen.print_char(b'\n');
    });
}
