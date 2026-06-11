//! High-level file system interface for FAT12.

use alloc::vec::Vec;
use core::fmt::Write;
use core::ops::ControlFlow;
use crate::drivers;
use crate::io::fat12::cluster::{deallocate_cluster_chain, fat12_next_cluster};
use crate::io::fat12::disk::{
    cluster_to_lba, read_fat_from_disk, read_root_directory_from_disk, write_fat_to_disk,
    write_root_directory_to_disk,
};
use crate::io::fat12::directory::{
    delete_file_entry, find_file_in_root_directory, for_each_active_root_entry,
    normalize_8_3_name,
};
use crate::io::fat12::types::{
    ATTR_DIRECTORY, BYTES_PER_SECTOR, DIRECTORY_ENTRY_SIZE, FAT12_BAD_CLUSTER, FAT12_EOF_MIN,
    FAT12_MAX_CLUSTER_ID, FAT12_MIN_DATA_CLUSTER, MAX_FILE_SIZE, ROOT_DIRECTORY_ENTRIES,
    EntryState, Fat12Error, FileEntryMeta, RawRootDirectoryEntry, RootDirectoryRecord,
};


/// Initialize the FAT12 file system.
///
/// Must be called after `drivers::ata::init()`.
///
/// The implementation is intentionally cache-free. Root directory data is
/// always read fresh from disk when requested.
pub fn init() {
    // No persistent state to initialize yet; kept as lifecycle hook for callers.
}

/// Read a file payload by following FAT12 cluster chain from directory metadata.
///
/// Invariants:
/// - `file_size` is treated as authoritative output length
/// - cluster chain must provide enough data to fill `file_size`
/// - chain cycles and malformed cluster values are rejected
pub fn read_file_from_entry(file_meta: FileEntryMeta, fat: &[u8]) -> Result<Vec<u8>, Fat12Error> {
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
