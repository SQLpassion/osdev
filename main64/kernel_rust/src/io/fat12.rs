//! FAT12 File System Driver
//!
//! Reads the FAT12 root directory from disk and provides functionality
//! to list its entries. The FAT12 layout matches a standard 1.44 MB
//! floppy disk image used by the KAOS bootloader.

use crate::drivers;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{Display, Formatter, Write};
use core::ops::ControlFlow;

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
const FAT1_LBA: u32 = RESERVED_SECTORS;
const DATA_AREA_START_LBA: u32 = ROOT_DIRECTORY_LBA + ROOT_DIRECTORY_SECTORS as u32;

// FAT12 root directory entry layout (all offsets within one 32-byte entry).
const DIRECTORY_ENTRY_SIZE: usize = 32;
const ATTR_OFFSET: usize = 11;
const FIRST_CLUSTER_OFFSET: usize = 26;
const FILE_SIZE_OFFSET: usize = 28;

// Attribute flags used by entry filtering.
const ATTR_LONG_FILE_NAME: u8 = 0x0F;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;

const FAT12_BAD_CLUSTER: u16 = 0x0FF7;
const FAT12_EOF_MIN: u16 = 0x0FF8;
const FAT12_MAX_CLUSTER_ID: usize = 0x1000;
const FAT12_MIN_DATA_CLUSTER: u16 = 2;

/// Maximum file size in bytes that will be accepted from FAT12 directory entries.
///
/// This limit protects against corrupted directory entries with unreasonably
/// large file_size fields that could cause heap exhaustion via Vec::with_capacity().
///
/// Set to 2 MiB, which is larger than the entire 1.44 MiB floppy capacity, but
/// small enough to prevent DoS attacks via memory exhaustion.
const MAX_FILE_SIZE: usize = 2 * 1024 * 1024;

/// FAT12-specific errors returned by directory parsing and file-content reads.
///
/// This enum separates low-level ATA failures from higher-level filesystem
/// semantics such as invalid names, missing entries, and FAT-chain corruption.
#[derive(Debug, Clone, Copy)]
pub enum Fat12Error {
    /// ATA controller or transport error while reading sectors.
    Ata(drivers::ata::AtaError),

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

impl From<drivers::ata::AtaError> for Fat12Error {
    fn from(value: drivers::ata::AtaError) -> Self {
        // Preserve original transport-layer failure while adapting to FAT12 API.
        Self::Ata(value)
    }
}

impl Display for Fat12Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ata(err) => write!(f, "ATA error: {:?}", err),
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

    fn short_name_raw(&self) -> [u8; 11] {
        // Return raw on-disk 8.3 bytes (already space padded) for exact matching.
        let mut short_name = [0u8; 11];
        short_name.copy_from_slice(&self.bytes[0..11]);
        short_name
    }

    fn attributes(&self) -> u8 {
        // Attributes byte contains file type/flags (DIR, ARCH, etc.).
        self.bytes[ATTR_OFFSET]
    }

    fn first_cluster(&self) -> u16 {
        // FAT12 stores first data cluster as little-endian u16 at offset 26.
        u16::from_le_bytes([
            self.bytes[FIRST_CLUSTER_OFFSET],
            self.bytes[FIRST_CLUSTER_OFFSET + 1],
        ])
    }

    fn file_size(&self) -> u32 {
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
struct FileEntryMeta {
    attributes: u8,
    first_cluster: u16,
    file_size: u32,
}

/// Initialize the FAT12 file system.
///
/// Must be called after `drivers::ata::init()`.
///
/// The implementation is intentionally cache-free. Root directory data is
/// always read fresh from disk when requested.
pub fn init() {
    // No persistent state to initialize yet; kept as lifecycle hook for callers.
}

/// Read the fixed-size FAT12 root directory area from disk.
///
/// For standard FAT12 this is always 14 sectors at LBA 19.
fn read_root_directory_from_disk() -> Result<Vec<u8>, Fat12Error> {
    // Allocate exactly the fixed root-directory window for this FAT12 geometry.
    let mut buffer = vec![0u8; ROOT_DIRECTORY_SECTORS as usize * BYTES_PER_SECTOR];
    drivers::ata::read_sectors(&mut buffer, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)?;

    Ok(buffer)
}

fn read_fat_from_disk() -> Result<Vec<u8>, Fat12Error> {
    // Read FAT#1 only; FAT#2 is a mirror copy used for redundancy on-disk.
    let mut buffer = vec![0u8; SECTORS_PER_FAT as usize * BYTES_PER_SECTOR];
    drivers::ata::read_sectors(&mut buffer, FAT1_LBA, SECTORS_PER_FAT as u8)?;

    Ok(buffer)
}

/// Normalize user-provided file name into FAT short-name storage layout.
///
/// Input examples:
/// - `"README.TXT"` -> `b"README  TXT"`
/// - `"KERNEL"` -> `b"KERNEL     "`
///
/// Rules enforced:
/// - base name length: 1..=8
/// - extension length: 1..=3 when `.` is present
/// - at most one `.` separator
/// - character set restricted to ASCII alnum plus `_` and `-`
/// - output uppercased and space-padded
pub fn normalize_8_3_name(file_name_8_3: &str) -> Result<[u8; 11], Fat12Error> {
    // Accept surrounding whitespace in shell input but validate the inner token strictly.
    let raw_name = file_name_8_3.trim();
    if raw_name.is_empty() {
        return Err(Fat12Error::InvalidFileName);
    }

    // Step 1: Split optional `base.ext` token and reject ambiguous separators.
    // FAT short names allow at most one '.' separator between base and extension.
    let mut parts = raw_name.split('.');
    let base = parts.next().ok_or(Fat12Error::InvalidFileName)?;
    let extension = parts.next();

    if parts.next().is_some() {
        return Err(Fat12Error::InvalidFileName);
    }

    if base.is_empty() || base.len() > 8 {
        return Err(Fat12Error::InvalidFileName);
    }

    if base.bytes().any(|b| !is_valid_short_name_char(b)) {
        return Err(Fat12Error::InvalidFileName);
    }

    // Step 2: Validate extension only when caller provided a separator.
    // A trailing dot (e.g. "KERNEL.") is rejected instead of being silently
    // normalized to "KERNEL".
    if let Some(ext) = extension {
        if ext.is_empty() {
            return Err(Fat12Error::InvalidFileName);
        }

        if ext.len() > 3 || ext.bytes().any(|b| !is_valid_short_name_char(b)) {
            return Err(Fat12Error::InvalidFileName);
        }
    }

    // Build canonical on-disk short-name representation: uppercased, space padded.
    let mut normalized = [b' '; 11];
    for (idx, b) in base.bytes().enumerate() {
        normalized[idx] = b.to_ascii_uppercase();
    }

    if let Some(ext) = extension {
        for (idx, b) in ext.bytes().enumerate() {
            normalized[8 + idx] = b.to_ascii_uppercase();
        }
    }

    Ok(normalized)
}

fn is_valid_short_name_char(b: u8) -> bool {
    // Conservative subset for 8.3 user input accepted by this implementation.
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Find a matching active short-name entry inside the root directory buffer.
///
/// The function scans fixed 32-byte entries and stops at FAT end marker `0x00`.
/// Deleted, LFN helper and volume-label entries are skipped via `state()`.
///
/// # Arguments
/// - `root_directory`: raw sector data read from LBA 19 (14 contiguous sectors,
///   7168 bytes total). The buffer may be shorter; only complete 32-byte slots
///   are examined.
/// - `normalized_name`: space-padded, uppercased 11-byte FAT short name
///   (8 bytes base + 3 bytes extension, no dot separator) as produced by
///   [`normalize_8_3_name`].
///
/// # Returns
/// - `Ok(FileEntryMeta)` — entry found; caller can inspect `attributes` to
///   distinguish regular files from directories.
/// - `Err(Fat12Error::NotFound)` — no active entry with a matching short name.
///
/// Structural metadata validation (e.g. reserved start-cluster checks) is
/// performed later by `read_file_from_entry()`.
fn find_file_in_root_directory(
    root_directory: &[u8],
    normalized_name: &[u8; 11],
) -> Result<FileEntryMeta, Fat12Error> {
    let mut found_entry = None;

    for_each_active_root_entry(root_directory, |entry| {
        // Compare raw 8.3 bytes directly against normalized short-name token.
        if &entry.short_name_raw() == normalized_name {
            found_entry = Some(FileEntryMeta {
                attributes: entry.attributes(),
                first_cluster: entry.first_cluster(),
                file_size: entry.file_size(),
            });

            return ControlFlow::Break(());
        }

        ControlFlow::Continue(())
    });

    found_entry.ok_or(Fat12Error::NotFound)
}

/// Iterate over active FAT12 root-directory entries with shared traversal semantics.
///
/// The iterator:
/// - parses only complete 32-byte slots up to FAT12 root entry capacity
/// - stops on FAT12 end marker (`EntryState::End`)
/// - skips deleted/LFN/volume-label slots (`EntryState::Skip`)
/// - forwards active entries to `on_active`
/// - allows early-exit via `ControlFlow::Break`
fn for_each_active_root_entry<F>(buffer: &[u8], mut on_active: F)
where
    F: FnMut(RawRootDirectoryEntry) -> ControlFlow<()>,
{
    // Parse at most the FAT12 root table size and never beyond provided bytes.
    let entry_count = core::cmp::min(ROOT_DIRECTORY_ENTRIES, buffer.len() / DIRECTORY_ENTRY_SIZE);

    for entry_idx in 0..entry_count {
        let start = entry_idx * DIRECTORY_ENTRY_SIZE;

        // Copy one raw slot into fixed local buffer for deterministic decode.
        let mut bytes = [0u8; DIRECTORY_ENTRY_SIZE];
        bytes.copy_from_slice(&buffer[start..start + DIRECTORY_ENTRY_SIZE]);

        let entry = RawRootDirectoryEntry { bytes };

        match entry.state() {
            EntryState::End => break,
            EntryState::Skip => continue,
            EntryState::Active => {
                if let ControlFlow::Break(()) = on_active(entry) {
                    break;
                }
            }
        }
    }
}

/// Decode next cluster ID from FAT12 table for the given current cluster.
///
/// FAT12 packs two 12-bit entries into 3 bytes. For a cluster `n`:
/// - offset = n + n / 2
/// - even `n`: low 12 bits of little-endian u16 at `offset`
/// - odd  `n`: high 12 bits of little-endian u16 at `offset`
fn fat12_next_cluster(fat: &[u8], cluster: u16) -> Result<u16, Fat12Error> {
    let offset = cluster as usize + (cluster as usize / 2);
    let byte0 = *fat.get(offset).ok_or(Fat12Error::CorruptFatChain)?;
    let byte1 = *fat.get(offset + 1).ok_or(Fat12Error::CorruptFatChain)?;
    let pair = u16::from_le_bytes([byte0, byte1]);

    // FAT12 stores two 12-bit entries in three bytes; even/odd cluster IDs decode differently.
    let value = if cluster & 1 == 0 {
        pair & 0x0FFF
    } else {
        (pair >> 4) & 0x0FFF
    };

    Ok(value)
}

/// Convert a FAT data-cluster index into absolute disk LBA.
///
/// Cluster numbering starts at 2 in FAT. Cluster 2 maps to first data sector.
fn cluster_to_lba(cluster: u16) -> Result<u32, Fat12Error> {
    if cluster < FAT12_MIN_DATA_CLUSTER {
        return Err(Fat12Error::CorruptFatChain);
    }

    Ok(DATA_AREA_START_LBA + (cluster as u32 - FAT12_MIN_DATA_CLUSTER as u32))
}

/// Read a file payload by following FAT12 cluster chain from directory metadata.
///
/// Invariants:
/// - `file_size` is treated as authoritative output length
/// - cluster chain must provide enough data to fill `file_size`
/// - chain cycles and malformed cluster values are rejected
fn read_file_from_entry(file_meta: FileEntryMeta, fat: &[u8]) -> Result<Vec<u8>, Fat12Error> {
    let file_size = file_meta.file_size as usize;
    // Zero-length files are valid and do not require FAT traversal.
    if file_size == 0 {
        return Ok(Vec::new());
    }

    // Reject unreasonably large file sizes that could cause heap exhaustion.
    if file_size > MAX_FILE_SIZE {
        return Err(Fat12Error::CorruptDirectoryEntry);
    }

    if file_meta.first_cluster < FAT12_MIN_DATA_CLUSTER {
        return Err(Fat12Error::CorruptDirectoryEntry);
    }

    let mut content = Vec::with_capacity(file_size);
    let mut current_cluster = file_meta.first_cluster;

    // FAT12 cluster namespace is 12-bit (0x000..0xFFF); this bitset detects cycles
    // with a fixed 512-byte stack footprint instead of 4 KiB for bool flags.
    let mut visited = [0u64; FAT12_MAX_CLUSTER_ID / 64];

    while content.len() < file_size {
        let cluster_index = current_cluster as usize;
        if cluster_index >= FAT12_MAX_CLUSTER_ID {
            return Err(Fat12Error::CorruptFatChain);
        }

        let visited_word = cluster_index / 64;
        let visited_mask = 1u64 << (cluster_index % 64);

        // Loop in FAT chain indicates corrupted allocation metadata.
        if visited[visited_word] & visited_mask != 0 {
            return Err(Fat12Error::CorruptFatChain);
        }
        visited[visited_word] |= visited_mask;

        // FAT12 on this media profile is 1 sector per cluster.
        let cluster_lba = cluster_to_lba(current_cluster)?;
        let mut sector = [0u8; BYTES_PER_SECTOR];
        drivers::ata::read_sectors(&mut sector, cluster_lba, 1)?;

        // The last cluster may contain trailing bytes beyond logical file size.
        let remaining = file_size - content.len();
        let copy_len = core::cmp::min(remaining, BYTES_PER_SECTOR);
        content.extend_from_slice(&sector[..copy_len]);

        if content.len() == file_size {
            break;
        }

        // Reject invalid data-chain targets before following them.
        let next_cluster = fat12_next_cluster(fat, current_cluster)?;

        if next_cluster <= 1
            || next_cluster == FAT12_BAD_CLUSTER
            || (0x0FF0..=0x0FF6).contains(&next_cluster)
        {
            return Err(Fat12Error::CorruptFatChain);
        }

        // EOF before enough payload bytes means directory size and FAT chain disagree.
        if next_cluster >= FAT12_EOF_MIN {
            return Err(Fat12Error::UnexpectedEof);
        }

        current_cluster = next_cluster;
    }

    Ok(content)
}

/// Read a regular file from FAT12 root directory by 8.3 name.
///
/// End-to-end flow:
/// 1. Normalize input into FAT short-name bytes.
/// 2. Read root directory and locate matching active entry.
/// 3. Reject directory entries.
/// 4. Read FAT#1 and follow cluster chain until `file_size` bytes are produced.
pub fn read_file(file_name_8_3: &str) -> Result<Vec<u8>, Fat12Error> {
    // Convert human input into canonical FAT short-name bytes first.
    let normalized_name = normalize_8_3_name(file_name_8_3)?;
    let root_directory = read_root_directory_from_disk()?;
    let file_meta = find_file_in_root_directory(&root_directory, &normalized_name)?;

    // `read_file` explicitly targets regular files in phase 1.
    if file_meta.attributes & ATTR_DIRECTORY != 0 {
        return Err(Fat12Error::IsDirectory);
    }

    // FAT#1 is the authoritative allocation map used for cluster-chain traversal.
    let fat = read_fat_from_disk()?;
    read_file_from_entry(file_meta, &fat)
}

/// Parse all visible root-directory entries and call `on_entry` for each one.
///
/// Returns `(file_count, total_size)` for printed summary output.
/// The summary counts/sums only regular files; directory entries are still
/// forwarded to `on_entry` so callers can render complete listings.
pub fn parse_root_directory<F>(buffer: &[u8], mut on_entry: F) -> (u32, u32)
where
    F: FnMut(RootDirectoryRecord),
{
    let mut file_count: u32 = 0;
    let mut total_size: u32 = 0;

    for_each_active_root_entry(buffer, |entry| {
        // Preserve full listing behavior for callers, then compute summary
        // metrics only for regular files to keep "N File(s)" semantically exact.
        let is_directory = (entry.attributes() & ATTR_DIRECTORY) != 0;
        let record = entry.parse_record();

        // Delegate record handling to caller (printing, collecting, etc.).
        on_entry(record);

        if !is_directory {
            file_count += 1;
            total_size += record.file_size;
        }

        ControlFlow::Continue(())
    });

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
    let root_dir = match read_root_directory_from_disk() {
        Ok(root_dir) => root_dir,
        Err(err) => {
            // Surface media/I/O problems directly on screen for operator feedback.
            drivers::screen::with_screen(|screen| {
                let _ = writeln!(screen, "FAT12 read error: {}", err);
            });
            return;
        }
    };

    drivers::screen::with_screen(|screen| {
        let (file_count, total_size) = parse_root_directory(&root_dir, |entry| {
            // Parsed name bytes are ASCII-normalized by parser; fallback kept defensive.
            let name = core::str::from_utf8(&entry.name[..entry.name_len]).unwrap_or("???");

            let _ = write!(screen, "{} bytes", entry.file_size);
            let _ = write!(screen, "\tStart Cluster: {}", entry.first_cluster);
            let _ = write!(screen, "\t{}", name);
            screen.print_char(b'\n');
        });

        // Match classic FAT listing footer with count and aggregated byte size.
        let _ = write!(screen, "\t\t{} File(s)", file_count);
        let _ = write!(screen, "\t{} bytes", total_size);
        screen.print_char(b'\n');
    });
}

/// Write the fixed-size FAT12 root directory area to disk.
///
/// For standard FAT12 this is always 14 sectors at LBA 19.
pub fn write_root_directory_to_disk(buffer: &[u8]) -> Result<(), Fat12Error> {
    assert_eq!(buffer.len(), ROOT_DIRECTORY_SECTORS as usize * BYTES_PER_SECTOR);
    drivers::ata::write_sectors(buffer, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)?;
    Ok(())
}

/// Write the FAT table from memory back to the disk.
///
/// Under FAT12 specifications, both FAT#1 (LBA 1) and FAT#2 (LBA 10) are
/// mirrors and must be written simultaneously.
pub fn write_fat_to_disk(fat_buffer: &[u8]) -> Result<(), Fat12Error> {
    assert_eq!(fat_buffer.len(), SECTORS_PER_FAT as usize * BYTES_PER_SECTOR);
    // Write FAT#1
    drivers::ata::write_sectors(fat_buffer, FAT1_LBA, SECTORS_PER_FAT as u8)?;
    // Write FAT#2 (LBA 10 = FAT1_LBA + SECTORS_PER_FAT)
    let fat2_lba = FAT1_LBA + SECTORS_PER_FAT;
    drivers::ata::write_sectors(fat_buffer, fat2_lba, SECTORS_PER_FAT as u8)?;
    Ok(())
}

/// Encode next cluster ID in FAT12 table for the given cluster.
///
/// Packs a 12-bit entry value into 3 bytes at calculated offset.
/// SAFETY:
/// - Accesses are bounds-checked against the FAT slice length.
fn fat12_write_cluster_entry(fat: &mut [u8], cluster: u16, next_cluster_val: u16) -> Result<(), Fat12Error> {
    let offset = cluster as usize + (cluster as usize / 2);
    if offset + 1 >= fat.len() {
        return Err(Fat12Error::CorruptFatChain);
    }
    let val_masked = next_cluster_val & 0x0FFF;

    let byte0 = fat[offset];
    let byte1 = fat[offset + 1];
    let mut pair = u16::from_le_bytes([byte0, byte1]);

    if cluster & 1 == 0 {
        pair = (pair & 0xF000) | val_masked;
    } else {
        pair = (pair & 0x000F) | (val_masked << 4);
    }

    let le_bytes = pair.to_le_bytes();
    fat[offset] = le_bytes[0];
    fat[offset + 1] = le_bytes[1];
    Ok(())
}

/// Scans the FAT for the next available unallocated cluster.
///
/// Unallocated clusters have an entry value of `0x000` in the FAT table.
fn find_next_free_cluster(fat: &[u8]) -> Option<u16> {
    // Standard data clusters on 1.44MB Floppy start at 2 up to 2847.
    for cluster in 2..2848 {
        if let Ok(next) = fat12_next_cluster(fat, cluster) {
            if next == 0 {
                return Some(cluster);
            }
        }
    }
    None
}

/// Allocates a new cluster for the file and links it to `current_cluster`.
///
/// Zeroes the disk sector to prevent old data leakage (security zeroing).
fn allocate_new_cluster(fat: &mut [u8], current_cluster: u16) -> Result<u16, Fat12Error> {
    let new_cluster = find_next_free_cluster(fat).ok_or(Fat12Error::CorruptFatChain)?;

    // Link current cluster if valid
    if current_cluster >= FAT12_MIN_DATA_CLUSTER {
        fat12_write_cluster_entry(fat, current_cluster, new_cluster)?;
    }

    // Set EOF marker
    fat12_write_cluster_entry(fat, new_cluster, 0xFFF)?;

    // Security Zeroing: overwrite sector of the new cluster with zero bytes
    let cluster_lba = cluster_to_lba(new_cluster)?;
    let empty_sector = [0u8; BYTES_PER_SECTOR];
    drivers::ata::write_sectors(&empty_sector, cluster_lba, 1)?;

    Ok(new_cluster)
}

/// Deallocates the cluster chain beginning at `start_cluster`.
///
/// Sets FAT entries to `0x000` and zero-initializes sectors.
fn deallocate_cluster_chain(fat: &mut [u8], start_cluster: u16) -> Result<(), Fat12Error> {
    if start_cluster < FAT12_MIN_DATA_CLUSTER {
        return Ok(());
    }

    let mut current_cluster = start_cluster;
    let empty_sector = [0u8; BYTES_PER_SECTOR];

    loop {
        let next_cluster = fat12_next_cluster(fat, current_cluster)?;

        // Free the current cluster
        fat12_write_cluster_entry(fat, current_cluster, 0x000)?;

        // Clear sector on disk
        if let Ok(cluster_lba) = cluster_to_lba(current_cluster) {
            let _ = drivers::ata::write_sectors(&empty_sector, cluster_lba, 1);
        }

        if !(FAT12_MIN_DATA_CLUSTER..FAT12_EOF_MIN).contains(&next_cluster) {
            break;
        }

        current_cluster = next_cluster;
    }

    Ok(())
}

/// Returns index of the first free directory slot in the root directory.
fn find_free_directory_slot(root_dir: &[u8]) -> Result<usize, Fat12Error> {
    let entry_count = core::cmp::min(ROOT_DIRECTORY_ENTRIES, root_dir.len() / DIRECTORY_ENTRY_SIZE);
    for entry_idx in 0..entry_count {
        let start = entry_idx * DIRECTORY_ENTRY_SIZE;
        let first_char = root_dir[start];
        if first_char == 0x00 || first_char == 0xE5 {
            return Ok(entry_idx);
        }
    }
    Err(Fat12Error::NotFound)
}

/// Formats and updates the system RTC date/time from BIB into FAT timestamp structure.
fn get_current_fat_date_time() -> (u16, u16) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `BIB_OFFSET` points to bootloader-populated BIOS info in low memory.
    let bib = unsafe { &*(crate::memory::bios::BIB_OFFSET as *const crate::memory::bios::BiosInformationBlock) };

    let year = if bib.year >= 1980 { (bib.year - 1980) as u16 } else { 0 };
    let month = (bib.month as u16).clamp(1, 12);
    let day = (bib.day as u16).clamp(1, 31);
    let fat_date = (year << 9) | (month << 5) | day;

    let hour = (bib.hour as u16).clamp(0, 23);
    let minute = (bib.minute as u16).clamp(0, 59);
    let second = (bib.second as u16).clamp(0, 59) / 2;
    let fat_time = (hour << 11) | (minute << 5) | second;

    (fat_date, fat_time)
}

/// Creates a new directory entry at `entry_idx`.
fn create_directory_entry(root_dir: &mut [u8], entry_idx: usize, normalized_name: &[u8; 11], first_cluster: u16) {
    let start = entry_idx * DIRECTORY_ENTRY_SIZE;
    let entry_bytes = &mut root_dir[start..start + DIRECTORY_ENTRY_SIZE];

    entry_bytes.fill(0);
    entry_bytes[0..11].copy_from_slice(normalized_name);
    entry_bytes[ATTR_OFFSET] = 0x00; // Archive / Normal file

    let first_cluster_bytes = first_cluster.to_le_bytes();
    entry_bytes[FIRST_CLUSTER_OFFSET] = first_cluster_bytes[0];
    entry_bytes[FIRST_CLUSTER_OFFSET + 1] = first_cluster_bytes[1];

    let (date, time) = get_current_fat_date_time();
    let date_bytes = date.to_le_bytes();
    let time_bytes = time.to_le_bytes();

    entry_bytes[14..16].copy_from_slice(&time_bytes);
    entry_bytes[16..18].copy_from_slice(&date_bytes);
    entry_bytes[18..20].copy_from_slice(&date_bytes);
    entry_bytes[22..24].copy_from_slice(&time_bytes);
    entry_bytes[24..26].copy_from_slice(&date_bytes);
}

/// Updates size and first cluster field in the directory entry.
fn update_file_entry(root_dir: &mut [u8], entry_idx: usize, file_size: u32, first_cluster: u16) {
    let start = entry_idx * DIRECTORY_ENTRY_SIZE;
    let entry_bytes = &mut root_dir[start..start + DIRECTORY_ENTRY_SIZE];

    let first_cluster_bytes = first_cluster.to_le_bytes();
    entry_bytes[FIRST_CLUSTER_OFFSET] = first_cluster_bytes[0];
    entry_bytes[FIRST_CLUSTER_OFFSET + 1] = first_cluster_bytes[1];

    let size_bytes = file_size.to_le_bytes();
    entry_bytes[FILE_SIZE_OFFSET..FILE_SIZE_OFFSET + 4].copy_from_slice(&size_bytes);

    let (date, time) = get_current_fat_date_time();
    let date_bytes = date.to_le_bytes();
    let time_bytes = time.to_le_bytes();

    entry_bytes[18..20].copy_from_slice(&date_bytes);
    entry_bytes[22..24].copy_from_slice(&time_bytes);
    entry_bytes[24..26].copy_from_slice(&date_bytes);
}

/// Sets first char of file name to 0xE5 (deleted marker).
fn delete_file_entry(root_dir: &mut [u8], entry_idx: usize) {
    let start = entry_idx * DIRECTORY_ENTRY_SIZE;
    root_dir[start] = 0xE5;
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

static FILE_DESCRIPTORS: crate::sync::spinlock::SpinLock<Vec<FileDescriptor>> =
    crate::sync::spinlock::SpinLock::new(Vec::new());

/// Open a file in FAT12. Returns the file descriptor ID on success.
pub fn open_file(file_name: &str, mode: FileMode) -> Result<usize, Fat12Error> {
    let normalized_name = normalize_8_3_name(file_name)?;
    let mut root_dir = read_root_directory_from_disk()?;
    let mut fat = read_fat_from_disk()?;

    let mut entry_index = None;
    for entry_idx in 0..ROOT_DIRECTORY_ENTRIES {
        let start = entry_idx * DIRECTORY_ENTRY_SIZE;
        let entry_bytes = &root_dir[start..start + DIRECTORY_ENTRY_SIZE];
        let entry = RawRootDirectoryEntry {
            bytes: {
                let mut b = [0u8; DIRECTORY_ENTRY_SIZE];
                b.copy_from_slice(entry_bytes);
                b
            }
        };

        match entry.state() {
            EntryState::End => break,
            EntryState::Skip => continue,
            EntryState::Active => {
                if entry.short_name_raw() == normalized_name {
                    entry_index = Some((entry_idx, entry.first_cluster(), entry.file_size(), entry.attributes()));
                    break;
                }
            }
        }
    }

    let mut fds = FILE_DESCRIPTORS.lock();
    let next_fd = fds.iter().map(|fd| fd.fd).max().unwrap_or(0) + 1;

    match mode {
        FileMode::Read => {
            let (idx, first_cluster, size, attr) = entry_index.ok_or(Fat12Error::NotFound)?;
            if attr & ATTR_DIRECTORY != 0 {
                return Err(Fat12Error::IsDirectory);
            }

            let fd_entry = FileDescriptor {
                fd: next_fd,
                file_name: normalized_name,
                mode,
                start_cluster: first_cluster,
                current_cluster: first_cluster,
                current_offset: 0,
                file_size: size,
                root_entry_index: idx,
            };
            fds.push(fd_entry);
            Ok(next_fd)
        }
        FileMode::Write => {
            let (idx, start_cluster) = if let Some((idx, first_cluster, _, _)) = entry_index {
                deallocate_cluster_chain(&mut fat, first_cluster)?;
                update_file_entry(&mut root_dir, idx, 0, 0);
                (idx, 0)
            } else {
                let idx = find_free_directory_slot(&root_dir)?;
                create_directory_entry(&mut root_dir, idx, &normalized_name, 0);
                (idx, 0)
            };

            write_root_directory_to_disk(&root_dir)?;
            write_fat_to_disk(&fat)?;

            let fd_entry = FileDescriptor {
                fd: next_fd,
                file_name: normalized_name,
                mode,
                start_cluster,
                current_cluster: start_cluster,
                current_offset: 0,
                file_size: 0,
                root_entry_index: idx,
            };
            fds.push(fd_entry);
            Ok(next_fd)
        }
        FileMode::Append => {
            let (idx, start_cluster, size) = if let Some((idx, first_cluster, size, _)) = entry_index {
                (idx, first_cluster, size)
            } else {
                let idx = find_free_directory_slot(&root_dir)?;
                create_directory_entry(&mut root_dir, idx, &normalized_name, 0);
                write_root_directory_to_disk(&root_dir)?;
                (idx, 0, 0)
            };

            let fd_entry = FileDescriptor {
                fd: next_fd,
                file_name: normalized_name,
                mode,
                start_cluster,
                current_cluster: start_cluster,
                current_offset: size,
                file_size: size,
                root_entry_index: idx,
            };
            fds.push(fd_entry);
            Ok(next_fd)
        }
    }
}

/// Closes the active file descriptor.
pub fn close_file(fd: usize) -> Result<(), Fat12Error> {
    let mut fds = FILE_DESCRIPTORS.lock();
    if let Some(pos) = fds.iter().position(|entry| entry.fd == fd) {
        fds.remove(pos);
        Ok(())
    } else {
        Err(Fat12Error::NotFound)
    }
}

/// Seeks to a specific offset within the file descriptor.
pub fn seek_file(fd: usize, offset: u32) -> Result<(), Fat12Error> {
    let mut fds = FILE_DESCRIPTORS.lock();
    if let Some(entry) = fds.iter_mut().find(|e| e.fd == fd) {
        if offset > entry.file_size {
            return Err(Fat12Error::UnexpectedEof);
        }
        entry.current_offset = offset;
        Ok(())
    } else {
        Err(Fat12Error::NotFound)
    }
}

/// Returns whether the file offset has reached the end of the file.
pub fn eof_file(fd: usize) -> Result<bool, Fat12Error> {
    let fds = FILE_DESCRIPTORS.lock();
    if let Some(entry) = fds.iter().find(|e| e.fd == fd) {
        Ok(entry.current_offset >= entry.file_size)
    } else {
        Err(Fat12Error::NotFound)
    }
}

/// Reads data from a file descriptor into `buffer`.
pub fn read_file_fd(fd: usize, buffer: &mut [u8]) -> Result<usize, Fat12Error> {
    let mut fds = FILE_DESCRIPTORS.lock();
    let entry = fds.iter_mut().find(|e| e.fd == fd).ok_or(Fat12Error::NotFound)?;

    if entry.current_offset >= entry.file_size {
        return Ok(0);
    }

    let bytes_to_read = core::cmp::min(buffer.len(), (entry.file_size - entry.current_offset) as usize);
    if bytes_to_read == 0 {
        return Ok(0);
    }

    let fat = read_fat_from_disk()?;
    let cluster_offset = (entry.current_offset as usize) / BYTES_PER_SECTOR;
    let byte_offset = (entry.current_offset as usize) % BYTES_PER_SECTOR;

    let mut current_cluster = entry.start_cluster;

    for _ in 0..cluster_offset {
        current_cluster = fat12_next_cluster(&fat, current_cluster)?;
        if !(FAT12_MIN_DATA_CLUSTER..FAT12_EOF_MIN).contains(&current_cluster) {
            return Err(Fat12Error::CorruptFatChain);
        }
    }

    let mut bytes_read = 0;
    let mut temp_buffer = [0u8; BYTES_PER_SECTOR];

    while bytes_read < bytes_to_read {
        let cluster_lba = cluster_to_lba(current_cluster)?;
        drivers::ata::read_sectors(&mut temp_buffer, cluster_lba, 1)?;

        let chunk_offset = if bytes_read == 0 { byte_offset } else { 0 };
        let chunk_len = core::cmp::min(bytes_to_read - bytes_read, BYTES_PER_SECTOR - chunk_offset);

        buffer[bytes_read..bytes_read + chunk_len].copy_from_slice(&temp_buffer[chunk_offset..chunk_offset + chunk_len]);

        bytes_read += chunk_len;
        entry.current_offset += chunk_len as u32;

        if bytes_read < bytes_to_read {
            current_cluster = fat12_next_cluster(&fat, current_cluster)?;
            if !(FAT12_MIN_DATA_CLUSTER..FAT12_EOF_MIN).contains(&current_cluster) {
                return Err(Fat12Error::CorruptFatChain);
            }
        }
    }

    entry.current_cluster = current_cluster;
    Ok(bytes_read)
}

/// Writes data from `buffer` into a file descriptor.
pub fn write_file_fd(fd: usize, buffer: &[u8]) -> Result<usize, Fat12Error> {
    let mut fds = FILE_DESCRIPTORS.lock();
    let entry = fds.iter_mut().find(|e| e.fd == fd).ok_or(Fat12Error::NotFound)?;

    if entry.mode == FileMode::Read {
        return Err(Fat12Error::IsDirectory);
    }

    let bytes_to_write = buffer.len();
    if bytes_to_write == 0 {
        return Ok(0);
    }

    let mut root_dir = read_root_directory_from_disk()?;
    let mut fat = read_fat_from_disk()?;

    let mut current_cluster = entry.start_cluster;
    let cluster_offset = (entry.current_offset as usize) / BYTES_PER_SECTOR;
    let byte_offset = (entry.current_offset as usize) % BYTES_PER_SECTOR;

    if current_cluster == 0 {
        current_cluster = allocate_new_cluster(&mut fat, 0)?;
        entry.start_cluster = current_cluster;
    } else {
        for _ in 0..cluster_offset {
            let mut next = fat12_next_cluster(&fat, current_cluster)?;
            if !(FAT12_MIN_DATA_CLUSTER..FAT12_EOF_MIN).contains(&next) {
                next = allocate_new_cluster(&mut fat, current_cluster)?;
            }
            current_cluster = next;
        }
    }

    let mut bytes_written = 0;
    let mut temp_buffer = [0u8; BYTES_PER_SECTOR];

    while bytes_written < bytes_to_write {
        let cluster_lba = cluster_to_lba(current_cluster)?;
        let chunk_offset = if bytes_written == 0 { byte_offset } else { 0 };
        let chunk_len = core::cmp::min(bytes_to_write - bytes_written, BYTES_PER_SECTOR - chunk_offset);

        if chunk_len < BYTES_PER_SECTOR {
            drivers::ata::read_sectors(&mut temp_buffer, cluster_lba, 1)?;
        }

        temp_buffer[chunk_offset..chunk_offset + chunk_len].copy_from_slice(&buffer[bytes_written..bytes_written + chunk_len]);
        drivers::ata::write_sectors(&temp_buffer, cluster_lba, 1)?;

        bytes_written += chunk_len;
        entry.current_offset += chunk_len as u32;

        if bytes_written < bytes_to_write {
            let mut next = fat12_next_cluster(&fat, current_cluster)?;
            if !(FAT12_MIN_DATA_CLUSTER..FAT12_EOF_MIN).contains(&next) {
                next = allocate_new_cluster(&mut fat, current_cluster)?;
            }
            current_cluster = next;
        }
    }

    entry.current_cluster = current_cluster;
    if entry.current_offset > entry.file_size {
        entry.file_size = entry.current_offset;
    }

    update_file_entry(&mut root_dir, entry.root_entry_index, entry.file_size, entry.start_cluster);
    write_root_directory_to_disk(&root_dir)?;
    write_fat_to_disk(&fat)?;

    Ok(bytes_written)
}

/// Deletes a file by its 8.3 name.
pub fn delete_file(file_name: &str) -> Result<(), Fat12Error> {
    let normalized_name = normalize_8_3_name(file_name)?;
    let mut root_dir = read_root_directory_from_disk()?;
    let mut fat = read_fat_from_disk()?;

    let mut entry_index = None;
    for entry_idx in 0..ROOT_DIRECTORY_ENTRIES {
        let start = entry_idx * DIRECTORY_ENTRY_SIZE;
        let entry_bytes = &root_dir[start..start + DIRECTORY_ENTRY_SIZE];
        let entry = RawRootDirectoryEntry {
            bytes: {
                let mut b = [0u8; DIRECTORY_ENTRY_SIZE];
                b.copy_from_slice(entry_bytes);
                b
            }
        };

        match entry.state() {
            EntryState::End => break,
            EntryState::Skip => continue,
            EntryState::Active => {
                if entry.short_name_raw() == normalized_name {
                    entry_index = Some((entry_idx, entry.first_cluster(), entry.attributes()));
                    break;
                }
            }
        }
    }

    let (idx, first_cluster, attr) = entry_index.ok_or(Fat12Error::NotFound)?;
    if attr & ATTR_DIRECTORY != 0 {
        return Err(Fat12Error::IsDirectory);
    }

    deallocate_cluster_chain(&mut fat, first_cluster)?;
    delete_file_entry(&mut root_dir, idx);

    write_root_directory_to_disk(&root_dir)?;
    write_fat_to_disk(&fat)?;

    Ok(())
}

