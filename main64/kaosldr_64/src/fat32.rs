//! Minimal read-only FAT32 reader for the 64-bit loader (no_std, no alloc).
//!
//! This mirrors the kernel's `io::fat32` logic but is written for the constrained
//! loader environment: it talks directly to the disk via ATA PIO (`crate::ata`),
//! uses fixed low-memory scratch buffers instead of the heap, and streams the kernel
//! image straight into `KERNEL_BUFFER`. Unlike FAT12 (used previously), the FAT is far
//! too large to load wholesale, so FAT sectors are read on demand and a single-sector
//! cache avoids re-reading the same FAT sector for sequential clusters.
//!
//! The legacy BIOS image is a FAT32 superfloppy, so the volume (and thus the BPB)
//! starts at LBA 0.

use crate::ata::read_sectors;

/// Marker value for "no FAT sector currently cached".
const NO_FAT_CACHE: u32 = 0xFFFF_FFFF;

/// First cluster value that denotes the end of a cluster chain (FAT32 EOC).
const EOC: u32 = 0x0FFF_FFF8;

/// Low-memory scratch buffer for the BPB and directory sectors (identity mapped).
const SECTOR_BUF: *mut u8 = 0x30000 as *mut u8;

/// Low-memory scratch buffer caching one FAT sector (identity mapped).
const FAT_BUF: *mut u8 = 0x30200 as *mut u8;

/// Destination for the kernel image (higher-half mapping of physical 0x100000).
pub const KERNEL_BUFFER: *mut u8 = 0xFFFF_8000_0010_0000 as *mut u8;

/// Geometry of the mounted FAT32 volume plus the FAT-sector cache state.
struct Fat32Reader {
    /// Number of sectors per cluster.
    sec_per_clus: u32,
    /// Absolute LBA where the first FAT begins.
    fat_start_lba: u32,
    /// Absolute LBA where the data region (cluster 2) begins.
    data_start_lba: u32,
    /// First cluster of the root directory.
    root_cluster: u32,
    /// LBA currently held in `FAT_BUF`, or `NO_FAT_CACHE` if empty.
    fat_cache_lba: u32,
}

/// Reads a little-endian `u16` from a raw byte buffer at the given offset.
///
/// # Safety
/// `p + off + 1` must be a valid, readable address.
unsafe fn rd_u16(p: *const u8, off: usize) -> u16 {
    let p = p.add(off);
    (*p as u16) | ((*p.add(1) as u16) << 8)
}

/// Reads a little-endian `u32` from a raw byte buffer at the given offset.
///
/// # Safety
/// `p + off + 3` must be a valid, readable address.
unsafe fn rd_u32(p: *const u8, off: usize) -> u32 {
    let p = p.add(off);
    (*p as u32)
        | ((*p.add(1) as u32) << 8)
        | ((*p.add(2) as u32) << 16)
        | ((*p.add(3) as u32) << 24)
}

impl Fat32Reader {
    /// Reads and parses the BPB from LBA 0 (superfloppy ⇒ part_lba = 0).
    ///
    /// # Safety
    /// The caller must ensure ATA PIO is ready and `SECTOR_BUF` is writable.
    unsafe fn mount() -> Result<Self, &'static str> {
        // SAFETY: SECTOR_BUF points to 512 bytes of identity-mapped scratch RAM.
        read_sectors(SECTOR_BUF, 0, 1);

        let bytes_per_sec = rd_u16(SECTOR_BUF, 0x0B) as u32;
        if bytes_per_sec != 512 {
            return Err("Unsupported sector size (expected 512)");
        }

        let sec_per_clus = *SECTOR_BUF.add(0x0D) as u32;
        let rsvd_sec_cnt = rd_u16(SECTOR_BUF, 0x0E) as u32;
        let num_fats = *SECTOR_BUF.add(0x10) as u32;
        let fat_sz_32 = rd_u32(SECTOR_BUF, 0x24);
        let root_cluster = rd_u32(SECTOR_BUF, 0x2C);

        if sec_per_clus == 0 || num_fats == 0 || fat_sz_32 == 0 {
            return Err("Invalid FAT32 BPB");
        }

        // part_lba is 0 for the superfloppy, so the absolute LBAs are derived directly.
        let fat_start_lba = rsvd_sec_cnt;
        let data_start_lba = fat_start_lba + num_fats * fat_sz_32;

        Ok(Self {
            sec_per_clus,
            fat_start_lba,
            data_start_lba,
            root_cluster,
            fat_cache_lba: NO_FAT_CACHE,
        })
    }

    /// Converts a cluster index into the absolute LBA of its first sector.
    fn cluster_to_lba(&self, cluster: u32) -> u32 {
        self.data_start_lba + (cluster - 2) * self.sec_per_clus
    }

    /// Returns the next cluster in the FAT chain after `cluster`.
    ///
    /// FAT sectors are read on demand; the most recently read one is cached in
    /// `FAT_BUF` so that walking sequential clusters does not re-read the same sector.
    ///
    /// # Safety
    /// The caller must ensure ATA PIO is ready and `FAT_BUF` is writable.
    unsafe fn next_cluster(&mut self, cluster: u32) -> u32 {
        let fat_offset = cluster * 4;
        let fat_sector = self.fat_start_lba + fat_offset / 512;
        let offset = (fat_offset % 512) as usize;

        if self.fat_cache_lba != fat_sector {
            // SAFETY: FAT_BUF points to 512 bytes of identity-mapped scratch RAM.
            read_sectors(FAT_BUF, fat_sector, 1);
            self.fat_cache_lba = fat_sector;
        }

        // The top 4 bits are reserved in FAT32 and must be masked off.
        rd_u32(FAT_BUF, offset) & 0x0FFF_FFFF
    }

    /// Searches the root directory for a file by its 8.3 name.
    /// Returns its first cluster on success.
    ///
    /// # Safety
    /// The caller must ensure ATA PIO is ready and the scratch buffers are writable.
    unsafe fn find_in_root(&mut self, name: &[u8; 11]) -> Option<u32> {
        let mut cluster = self.root_cluster;
        let mut guard = 0u32;

        while cluster >= 2 && cluster < EOC {
            guard += 1;
            if guard > 100_000 {
                return None;
            }

            let base = self.cluster_to_lba(cluster);
            for s in 0..self.sec_per_clus {
                // SAFETY: SECTOR_BUF holds 512 bytes; reads one directory sector.
                read_sectors(SECTOR_BUF, base + s, 1);

                // Each 512-byte sector holds 16 directory entries of 32 bytes each.
                for e in 0..16 {
                    let off = e * 32;
                    let first = *SECTOR_BUF.add(off);

                    if first == 0x00 {
                        // End-of-directory marker: the file does not exist.
                        return None;
                    }
                    if first == 0xE5 {
                        // Deleted entry.
                        continue;
                    }
                    let attr = *SECTOR_BUF.add(off + 0x0B);
                    if attr == 0x0F {
                        // Long File Name (LFN) component entry, skip.
                        continue;
                    }

                    // Compare the raw 11-byte 8.3 name.
                    let mut matches = true;
                    for i in 0..11 {
                        if *SECTOR_BUF.add(off + i) != name[i] {
                            matches = false;
                            break;
                        }
                    }
                    if matches {
                        let hi = rd_u16(SECTOR_BUF, off + 0x14) as u32;
                        let lo = rd_u16(SECTOR_BUF, off + 0x1A) as u32;
                        return Some((hi << 16) | lo);
                    }
                }
            }

            cluster = self.next_cluster(cluster);
        }

        None
    }
}

/// Loads the given kernel file into memory at `KERNEL_BUFFER`.
/// Returns the number of 512-byte sectors written.
///
/// # Safety
/// The caller must ensure ATA PIO is ready, the scratch buffers and `KERNEL_BUFFER`
/// are writable, and that the disk holds a valid FAT32 superfloppy.
pub unsafe fn load_kernel_into_memory(filename: &[u8; 11]) -> Result<i32, &'static str> {
    let mut reader = Fat32Reader::mount()?;

    let first_cluster = reader
        .find_in_root(filename)
        .ok_or("Kernel file not found")?;
    if first_cluster < 2 {
        return Err("Kernel file has invalid first cluster");
    }

    let mut cluster = first_cluster;
    let mut dst = KERNEL_BUFFER;
    let mut sector_count: i32 = 0;
    let mut guard = 0u32;

    // Walk the file's cluster chain, reading each whole cluster into memory.
    while cluster >= 2 && cluster < EOC {
        guard += 1;
        if guard > 10_000_000 {
            return Err("Corrupted FAT chain");
        }

        let base = reader.cluster_to_lba(cluster);

        // SAFETY: sec_per_clus fits in a u8 for all realistic volumes; `dst` advances
        // by exactly the number of bytes written.
        read_sectors(dst, base, reader.sec_per_clus as u8);
        dst = dst.add((reader.sec_per_clus as usize) * 512);
        sector_count += reader.sec_per_clus as i32;

        cluster = reader.next_cluster(cluster);
    }

    Ok(sector_count)
}
