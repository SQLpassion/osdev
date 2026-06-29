//! Common types, errors, and constants for the FAT12 driver.

use core::fmt::{Display, Formatter};

// FAT12 disk geometry constants for a 1.44 MB floppy layout.
pub const BYTES_PER_SECTOR: usize = 512;
pub const FAT_COUNT: u32 = 2;
pub const SECTORS_PER_FAT: u32 = 9;
pub const RESERVED_SECTORS: u32 = 1;
pub const ROOT_DIRECTORY_ENTRIES: usize = 224;

/// LBA address of the root directory: FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS = 19
pub const ROOT_DIRECTORY_LBA: u32 = FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS;

/// Number of sectors occupied by the root directory: 32 * 224 / 512 = 14
pub const ROOT_DIRECTORY_SECTORS: u8 = (32 * ROOT_DIRECTORY_ENTRIES / BYTES_PER_SECTOR) as u8;
pub const FAT1_LBA: u32 = RESERVED_SECTORS;
pub const DATA_AREA_START_LBA: u32 = ROOT_DIRECTORY_LBA + ROOT_DIRECTORY_SECTORS as u32;

// FAT12 root directory entry layout (all offsets within one 32-byte entry).
pub const DIRECTORY_ENTRY_SIZE: usize = 32;
pub const ATTR_OFFSET: usize = 11;
pub const FIRST_CLUSTER_OFFSET: usize = 26;
pub const FILE_SIZE_OFFSET: usize = 28;

// Attribute flags used by entry filtering.
pub const ATTR_LONG_FILE_NAME: u8 = 0x0F;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;

pub const FAT12_BAD_CLUSTER: u16 = 0x0FF7;
pub const FAT12_EOF_MIN: u16 = 0x0FF8;
pub const FAT12_MAX_CLUSTER_ID: usize = 0x1000;
pub const FAT12_MIN_DATA_CLUSTER: u16 = 2;

/// Maximum file size in bytes that will be accepted from FAT12 directory entries.
///
/// This limit protects against corrupted directory entries with unreasonably
/// large file_size fields that could cause heap exhaustion via Vec::with_capacity().
///
/// Set to 2 MiB, which is larger than the entire 1.44 MiB floppy capacity, but
/// small enough to prevent DoS attacks via memory exhaustion.
pub const MAX_FILE_SIZE: usize = 2 * 1024 * 1024;

/// FAT12-specific errors returned by directory parsing and file-content reads.
///
/// This enum separates low-level ATA failures from higher-level filesystem
/// semantics such as invalid names, missing entries, and FAT-chain corruption.
#[derive(Debug, Clone, Copy)]
pub enum Fat12Error {
    /// Block device error while reading/writing sectors.
    Block(crate::drivers::block::BlockError),

    /// Input file name is not representable as a valid FAT 8.3 short name.
    InvalidFileName,

    /// Requested short-name entry does not exist in the root directory.
    NotFound,

    /// Matched root-directory entry is a directory, not a regular file.
    IsDirectory,

    /// Root-directory metadata is structurally invalid (e.g. bad start cluster).
    CorruptDirectoryEntry,

    /// FAT table chain is malformed (loop, reserved/bad/out-of-range value).
    CorruptFatChain,

    /// FAT chain ended before `file_size` bytes could be read.
    UnexpectedEof,
}

impl From<crate::drivers::block::BlockError> for Fat12Error {
    fn from(value: crate::drivers::block::BlockError) -> Self {
        // Preserve original transport-layer failure while adapting to FAT12 API.
        Self::Block(value)
    }
}

impl Display for Fat12Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Block(err) => write!(f, "Block error: {:?}", err),
            Self::InvalidFileName => f.write_str("invalid FAT 8.3 file name"),
            Self::NotFound => f.write_str("file not found in FAT12 root directory"),
            Self::IsDirectory => f.write_str("entry is a directory, not a regular file"),
            Self::CorruptDirectoryEntry => f.write_str("corrupt FAT12 directory entry"),
            Self::CorruptFatChain => f.write_str("corrupt FAT12 cluster chain"),
            Self::UnexpectedEof => f.write_str("unexpected FAT12 EOF before file size completed"),
        }
    }
}

/// Parsed root-directory entry in a format convenient for shell output/tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootDirectoryRecord {
    pub name: [u8; 13],
    pub name_len: usize,
    pub first_cluster: u16,
    pub file_size: u32,
}

#[derive(Clone, Copy)]
pub enum EntryState {
    /// Entry byte 0 is 0x00: no more entries in this directory table.
    End,

    /// Entry exists in table but should not be exposed by `dir`.
    Skip,

    /// Normal short (8.3) file/directory entry.
    Active,
}

/// Raw 32-byte on-disk FAT12 root directory entry.
#[derive(Clone, Copy)]
pub struct RawRootDirectoryEntry {
    pub bytes: [u8; DIRECTORY_ENTRY_SIZE],
}

impl RawRootDirectoryEntry {
    /// Classify this raw entry according to FAT12 root-directory rules.
    pub fn state(&self) -> EntryState {
        let first_name_byte = self.bytes[0];

        // 0x00 means "no more used entries after this slot".
        if first_name_byte == 0x00 {
            return EntryState::End;
        }

        // 0xE5 marks a deleted directory entry.
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
    pub fn parse_record(&self) -> RootDirectoryRecord {
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

        // Add `.ext` only when an extension is actually present.
        if extension.iter().any(|&b| b != b' ') {
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

        let first_cluster = self.first_cluster();
        let file_size = self.file_size();

        RootDirectoryRecord {
            name,
            name_len: pos,
            first_cluster,
            file_size,
        }
    }

    pub fn short_name_raw(&self) -> [u8; 11] {
        // Return raw on-disk 8.3 bytes (already space padded) for exact matching.
        let mut short_name = [0u8; 11];
        short_name.copy_from_slice(&self.bytes[0..11]);
        short_name
    }

    pub fn attributes(&self) -> u8 {
        // Attributes byte contains file type/flags (DIR, ARCH, etc.).
        self.bytes[ATTR_OFFSET]
    }

    pub fn first_cluster(&self) -> u16 {
        // FAT12 stores first data cluster as little-endian u16 at offset 26.
        u16::from_le_bytes([
            self.bytes[FIRST_CLUSTER_OFFSET],
            self.bytes[FIRST_CLUSTER_OFFSET + 1],
        ])
    }

    pub fn file_size(&self) -> u32 {
        // Logical file size in bytes from directory entry offset 28.
        u32::from_le_bytes([
            self.bytes[FILE_SIZE_OFFSET],
            self.bytes[FILE_SIZE_OFFSET + 1],
            self.bytes[FILE_SIZE_OFFSET + 2],
            self.bytes[FILE_SIZE_OFFSET + 3],
        ])
    }
}

#[derive(Clone, Copy)]
pub struct FileEntryMeta {
    pub attributes: u8,
    pub first_cluster: u16,
    pub file_size: u32,
}

/// File opening mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    Append,
}

/// File descriptor instance inside the active FD table.
pub struct FileDescriptor {
    pub fd: usize,
    #[allow(dead_code)]
    pub file_name: [u8; 11],
    pub mode: FileMode,
    pub start_cluster: u16,
    pub current_cluster: u16,
    pub current_offset: u32,
    pub file_size: u32,
    pub root_entry_index: usize,
}
