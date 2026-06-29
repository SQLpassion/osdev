//! Cluster chain management functions for FAT12.

use crate::drivers::block;
use crate::io::fat12::disk::cluster_to_lba;
use crate::io::fat12::types::{
    Fat12Error, BYTES_PER_SECTOR, FAT12_EOF_MIN, FAT12_MIN_DATA_CLUSTER,
};

/// Decode next cluster ID from FAT12 table for the given current cluster.
///
/// FAT12 packs two 12-bit entries into 3 bytes. For a cluster `n`:
/// - offset = n + n / 2
/// - even `n`: low 12 bits of little-endian u16 at `offset`
/// - odd  `n`: high 12 bits of little-endian u16 at `offset`
pub fn fat12_next_cluster(fat: &[u8], cluster: u16) -> Result<u16, Fat12Error> {
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

/// Encode next cluster ID in FAT12 table for the given cluster.
///
/// Packs a 12-bit entry value into 3 bytes at calculated offset.
/// SAFETY:
/// - Accesses are bounds-checked against the FAT slice length.
pub fn fat12_write_cluster_entry(
    fat: &mut [u8],
    cluster: u16,
    next_cluster_val: u16,
) -> Result<(), Fat12Error> {
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
pub fn find_next_free_cluster(fat: &[u8]) -> Option<u16> {
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
pub fn allocate_new_cluster(fat: &mut [u8], current_cluster: u16) -> Result<u16, Fat12Error> {
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
    block::write_sectors(cluster_lba as u64, 1, &empty_sector)?;

    Ok(new_cluster)
}

/// Deallocates the cluster chain beginning at `start_cluster`.
///
/// Sets FAT entries to `0x000` and zero-initializes sectors.
pub fn deallocate_cluster_chain(fat: &mut [u8], start_cluster: u16) -> Result<(), Fat12Error> {
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
            let _ = block::write_sectors(cluster_lba as u64, 1, &empty_sector);
        }

        if !(FAT12_MIN_DATA_CLUSTER..FAT12_EOF_MIN).contains(&next_cluster) {
            break;
        }

        current_cluster = next_cluster;
    }

    Ok(())
}
