//! Low-level FAT12 disk I/O helper functions.

use crate::drivers::block;
use crate::io::fat12::types::{
    Fat12Error, BYTES_PER_SECTOR, DATA_AREA_START_LBA, FAT12_MIN_DATA_CLUSTER, FAT1_LBA,
    ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS, SECTORS_PER_FAT,
};
use alloc::vec;
use alloc::vec::Vec;

/// Read the fixed-size FAT12 root directory area from disk.
///
/// For standard FAT12 this is always 14 sectors at LBA 19.
pub fn read_root_directory_from_disk() -> Result<Vec<u8>, Fat12Error> {
    // Allocate exactly the fixed root-directory window for this FAT12 geometry.
    let mut buffer = vec![0u8; ROOT_DIRECTORY_SECTORS as usize * BYTES_PER_SECTOR];
    block::read_sectors(
        ROOT_DIRECTORY_LBA as u64,
        ROOT_DIRECTORY_SECTORS as u32,
        &mut buffer,
    )?;

    Ok(buffer)
}

pub fn read_fat_from_disk() -> Result<Vec<u8>, Fat12Error> {
    // Read FAT#1 only; FAT#2 is a mirror copy used for redundancy on-disk.
    let mut buffer = vec![0u8; SECTORS_PER_FAT as usize * BYTES_PER_SECTOR];
    block::read_sectors(FAT1_LBA as u64, SECTORS_PER_FAT, &mut buffer)?;

    Ok(buffer)
}

/// Write the fixed-size FAT12 root directory area to disk.
///
/// For standard FAT12 this is always 14 sectors at LBA 19.
pub fn write_root_directory_to_disk(buffer: &[u8]) -> Result<(), Fat12Error> {
    assert_eq!(
        buffer.len(),
        ROOT_DIRECTORY_SECTORS as usize * BYTES_PER_SECTOR
    );
    block::write_sectors(
        ROOT_DIRECTORY_LBA as u64,
        ROOT_DIRECTORY_SECTORS as u32,
        buffer,
    )?;
    Ok(())
}

/// Write the FAT table from memory back to the disk.
///
/// Under FAT12 specifications, both FAT#1 (LBA 1) and FAT#2 (LBA 10) are
/// mirrors and must be written simultaneously.
pub fn write_fat_to_disk(fat_buffer: &[u8]) -> Result<(), Fat12Error> {
    assert_eq!(
        fat_buffer.len(),
        SECTORS_PER_FAT as usize * BYTES_PER_SECTOR
    );
    // Write FAT#1
    block::write_sectors(FAT1_LBA as u64, SECTORS_PER_FAT, fat_buffer)?;
    // Write FAT#2 (LBA 10 = FAT1_LBA + SECTORS_PER_FAT)
    let fat2_lba = FAT1_LBA + SECTORS_PER_FAT;
    block::write_sectors(fat2_lba as u64, SECTORS_PER_FAT, fat_buffer)?;
    Ok(())
}

/// Convert a FAT data-cluster index into absolute disk LBA.
///
/// Cluster numbering starts at 2 in FAT. Cluster 2 maps to first data sector.
pub fn cluster_to_lba(cluster: u16) -> Result<u32, Fat12Error> {
    if cluster < FAT12_MIN_DATA_CLUSTER {
        return Err(Fat12Error::CorruptFatChain);
    }

    Ok(DATA_AREA_START_LBA + (cluster as u32 - FAT12_MIN_DATA_CLUSTER as u32))
}
