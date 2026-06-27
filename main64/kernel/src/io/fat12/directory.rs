//! Directory-related operations and entry manipulation for FAT12.

use crate::io::fat12::types::{
    EntryState, Fat12Error, FileEntryMeta, RawRootDirectoryEntry, ATTR_OFFSET,
    DIRECTORY_ENTRY_SIZE, FILE_SIZE_OFFSET, FIRST_CLUSTER_OFFSET, ROOT_DIRECTORY_ENTRIES,
};
use core::ops::ControlFlow;

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
pub fn find_file_in_root_directory(
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
pub fn for_each_active_root_entry<F>(buffer: &[u8], mut on_active: F)
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

/// Returns index of the first free directory slot in the root directory.
pub fn find_free_directory_slot(root_dir: &[u8]) -> Result<usize, Fat12Error> {
    let entry_count = core::cmp::min(
        ROOT_DIRECTORY_ENTRIES,
        root_dir.len() / DIRECTORY_ENTRY_SIZE,
    );
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
pub fn get_current_fat_date_time() -> (u16, u16) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `BIB_OFFSET` points to bootloader-populated BIOS info in low memory.
    let bib = unsafe {
        &*(crate::memory::bios::BIB_OFFSET as *const crate::memory::bios::BiosInformationBlock)
    };

    let year = if bib.year >= 1980 {
        (bib.year - 1980) as u16
    } else {
        0
    };
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
pub fn create_directory_entry(
    root_dir: &mut [u8],
    entry_idx: usize,
    normalized_name: &[u8; 11],
    first_cluster: u16,
) {
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
pub fn update_file_entry(
    root_dir: &mut [u8],
    entry_idx: usize,
    file_size: u32,
    first_cluster: u16,
) {
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
pub fn delete_file_entry(root_dir: &mut [u8], entry_idx: usize) {
    let start = entry_idx * DIRECTORY_ENTRY_SIZE;
    root_dir[start] = 0xE5;
}
