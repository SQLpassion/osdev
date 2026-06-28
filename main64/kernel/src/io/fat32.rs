use alloc::vec::Vec;

/// Represents a mounted, read-only FAT32 volume.
///
/// This struct holds the necessary geometric parameters derived from the BIOS Parameter Block (BPB)
/// to translate FAT32 cluster indices into raw disk Logical Block Addresses (LBAs).
pub struct Fat32Volume {
    /// The starting LBA of the partition (e.g., the EFI System Partition).
    #[allow(dead_code)]
    part_lba: u64,

    /// The number of bytes per sector. For this implementation, this is strictly 512.
    #[allow(dead_code)]
    bytes_per_sec: u32,

    /// The number of sectors mapped to a single FAT cluster.
    sec_per_clus: u32,

    /// The absolute LBA where the File Allocation Tables (FAT) begin.
    fat_start_lba: u64,

    /// The absolute LBA where the actual file data clusters begin.
    data_start_lba: u64,

    /// The first cluster index of the root directory.
    root_cluster: u32,
}

/// Errors that can occur during FAT32 operations.
#[derive(Debug, Clone, Copy)]
pub enum Fat32Error {
    /// An underlying disk/AHCI read error occurred.
    Ahci,

    /// The volume does not conform to the expected FAT32 structure (e.g., bad signature).
    NotFat32,

    /// The requested file was not found in the root directory.
    NotFound,

    /// The requested name is a directory, not a readable file.
    IsDirectory,

    /// A loop or structurally invalid cluster was encountered in the FAT chain.
    BadChain,

    /// The file exceeds the defensively defined maximum size limit (e.g., > 8 MiB).
    TooLarge,
}

impl Fat32Volume {
    /// Mounts the FAT32 volume at the given base LBA.
    // SAFETY:
    // - reads BPB from the first sector of the partition
    // - validates the structural requirements of FAT32 (signatures, counts)
    pub fn mount(part_lba: u64) -> Result<Self, Fat32Error> {
        let mut sector = [0u8; 512];

        // Step 1: Read the BPB
        // We read the first sector of the partition (the BIOS Parameter Block) via AHCI
        // to extract the filesystem geometry. If the read fails, we abort.
        crate::drivers::ahci::read_sectors(&mut sector, part_lba as u32, 1)
            .map_err(|_| Fat32Error::Ahci)?;

        // Step 2: Validate FAT32 parameters
        // We verify that the sector size is exactly 512 bytes, as our implementation
        // strictly relies on this assumption.
        let bytes_per_sec = u16::from_le_bytes(sector[0x0B..0x0D].try_into().unwrap()) as u32;
        if bytes_per_sec != 512 {
            return Err(Fat32Error::NotFat32);
        }

        let sec_per_clus = sector[0x0D] as u32;
        let rsvd_sec_cnt = u16::from_le_bytes(sector[0x0E..0x10].try_into().unwrap()) as u32;
        let num_fats = sector[0x10] as u32;
        let root_ent_cnt = u16::from_le_bytes(sector[0x11..0x13].try_into().unwrap());
        let fat_sz_16 = u16::from_le_bytes(sector[0x16..0x18].try_into().unwrap());
        let fat_sz_32 = u32::from_le_bytes(sector[0x24..0x28].try_into().unwrap());
        let root_cluster = u32::from_le_bytes(sector[0x2C..0x30].try_into().unwrap());
        let signature = u16::from_le_bytes(sector[0x1FE..0x200].try_into().unwrap());

        // Ensure this is actually a FAT32 volume by checking that RootEntCnt and FATSz16 are 0,
        // which are FAT12/FAT16 specific, and that the boot sector signature is present.
        if root_ent_cnt != 0 || fat_sz_16 != 0 || signature != 0xAA55 {
            return Err(Fat32Error::NotFat32);
        }

        // Step 3: Compute important LBAs for FAT32
        // Calculate the absolute LBA for the start of the FAT (skipping reserved sectors)
        // and the start of the data region (skipping reserved sectors and all FAT copies).
        let fat_start_lba = part_lba + rsvd_sec_cnt as u64;
        let data_start_lba = fat_start_lba + (num_fats * fat_sz_32) as u64;

        crate::debugln!(
            "FAT32 mounted: part_lba={}, fat_start={}, data_start={}, sec_per_clus={}",
            part_lba,
            fat_start_lba,
            data_start_lba,
            sec_per_clus
        );

        Ok(Self {
            part_lba,
            bytes_per_sec,
            sec_per_clus,
            fat_start_lba,
            data_start_lba,
            root_cluster,
        })
    }

    /// Reads a file from the root directory into a newly allocated buffer.
    // SAFETY:
    // - normalizes the requested file name
    // - follows the root directory cluster chain
    // - matches file entries securely and allocates exactly the required bytes
    // - respects maximum file sizes and prevents infinite loops on bad chains
    pub fn read_file(&self, name: &str) -> Result<Vec<u8>, Fat32Error> {
        // Step 1: Build the 8.3 match key
        // Convert the requested file name to the standard 11-byte space-padded uppercase
        // 8.3 format (e.g., "SHELL   BIN") so we can directly match it against directory entries.
        let key = normalize_name(name).ok_or(Fat32Error::NotFound)?;

        let mut current_cluster = self.root_cluster;
        let mut target_first_cluster = 0;
        let mut target_file_size = 0;
        let mut found = false;

        // Step 2: Walk the root-directory cluster chain
        // We traverse the directory clusters to find our target file. We use a safety
        // counter `cluster_count` to prevent infinite loops in case the FAT chain is corrupted.
        let mut cluster_count = 0;
        'dir_walk: while (2..0x0FFF_FFF8).contains(&current_cluster) {
            cluster_count += 1;
            if cluster_count > 1_000_000 {
                return Err(Fat32Error::BadChain);
            }

            // For each cluster, we read all its sectors sequentially.
            let cluster_lba = self.cluster_to_lba(current_cluster);
            for i in 0..self.sec_per_clus {
                let mut sector = [0u8; 512];
                crate::drivers::ahci::read_sectors(&mut sector, (cluster_lba + i as u64) as u32, 1)
                    .map_err(|_| Fat32Error::Ahci)?;

                // Process 32-byte directory entries in the sector
                // Each sector can hold exactly 16 (512/32) directory entries.
                for entry_idx in 0..(512 / 32) {
                    let offset = entry_idx * 32;
                    let first_byte = sector[offset];
                    if first_byte == 0x00 {
                        // End of directory marker reached, early return.
                        break 'dir_walk;
                    }
                    if first_byte == 0xE5 {
                        // Deleted entry, skip.
                        continue;
                    }
                    let attr = sector[offset + 0x0B];
                    if attr == 0x0F {
                        // LFN (Long File Name) entry, skip.
                        continue;
                    }

                    if sector[offset..offset + 11] == key {
                        // Found the entry matching our 8.3 name key.
                        // Reject directories since this function only reads files.
                        if attr & 0x10 != 0 {
                            return Err(Fat32Error::IsDirectory);
                        }

                        // Reconstruct the 32-bit first cluster from the high and low 16-bit words.
                        let clus_hi = u16::from_le_bytes(
                            sector[offset + 0x14..offset + 0x16].try_into().unwrap(),
                        ) as u32;
                        let clus_lo = u16::from_le_bytes(
                            sector[offset + 0x1A..offset + 0x1C].try_into().unwrap(),
                        ) as u32;

                        target_first_cluster = (clus_hi << 16) | clus_lo;
                        target_file_size = u32::from_le_bytes(
                            sector[offset + 0x1C..offset + 0x20].try_into().unwrap(),
                        );

                        found = true;
                        break 'dir_walk;
                    }
                }
            }

            // Look up the next cluster index in the FAT to continue the directory walk.
            current_cluster = self.next_cluster(current_cluster)?;
        }

        if !found {
            return Err(Fat32Error::NotFound);
        }

        // Step 3: Bound file_size
        // Defensively enforce a maximum file size (8 MiB) to prevent exhausting available
        // memory during the read phase.
        if target_file_size > 8 * 1024 * 1024 {
            return Err(Fat32Error::TooLarge);
        }

        // Step 4: Walk the file's cluster chain
        // We follow the file's cluster chain, copying data into our `Vec` until we have
        // read exactly `target_file_size` bytes. The loop is similarly guarded against corruption.
        let mut content = Vec::with_capacity(target_file_size as usize);
        let mut current_cluster = target_first_cluster;
        let mut cluster_count = 0;

        while (2..0x0FFF_FFF8).contains(&current_cluster) {
            cluster_count += 1;
            if cluster_count > 1_000_000 {
                return Err(Fat32Error::BadChain);
            }

            // For each cluster, read its sectors and append their bytes to our buffer.
            let cluster_lba = self.cluster_to_lba(current_cluster);
            for i in 0..self.sec_per_clus {
                let mut sector = [0u8; 512];
                crate::drivers::ahci::read_sectors(&mut sector, (cluster_lba + i as u64) as u32, 1)
                    .map_err(|_| Fat32Error::Ahci)?;

                // Only copy the bytes that actually belong to the file, to avoid reading
                // padding zeros from the last sector.
                let remaining_bytes = (target_file_size as usize) - content.len();
                if remaining_bytes == 0 {
                    break;
                }

                let to_copy = core::cmp::min(remaining_bytes, 512);
                content.extend_from_slice(&sector[..to_copy]);
            }

            if content.len() >= target_file_size as usize {
                break;
            }

            // Look up the next cluster index in the FAT to continue reading the file.
            current_cluster = self.next_cluster(current_cluster)?;
        }

        // Step 5: Return the populated Vec<u8>
        Ok(content)
    }

    /// Helper to translate a cluster number to its first LBA.
    fn cluster_to_lba(&self, cluster: u32) -> u64 {
        self.data_start_lba + (cluster as u64 - 2) * self.sec_per_clus as u64
    }

    /// Helper to look up the next cluster in the FAT chain.
    // SAFETY:
    // - computes the FAT sector and offset correctly for FAT32
    // - reads the sector and interprets the 32-bit entry securely
    fn next_cluster(&self, cluster: u32) -> Result<u32, Fat32Error> {
        // Step 1: Compute FAT sector and byte offset
        // Each cluster entry in FAT32 is 4 bytes. We calculate the sector containing
        // the entry and its byte offset within that sector.
        let fat_sector = self.fat_start_lba + (cluster * 4) as u64 / 512;
        let offset = (cluster * 4) as usize % 512;

        // Step 2: Read the FAT sector
        // We perform a read via AHCI to retrieve the specific FAT sector.
        let mut sector = [0u8; 512];
        crate::drivers::ahci::read_sectors(&mut sector, fat_sector as u32, 1)
            .map_err(|_| Fat32Error::Ahci)?;

        // Step 3: Extract and validate the next cluster
        // Mask out the top 4 bits (which are reserved in FAT32) and interpret the result.
        let value =
            u32::from_le_bytes(sector[offset..offset + 4].try_into().unwrap()) & 0x0FFF_FFFF;

        // Check for bad clusters or valid end-of-chain markers. Any value < 2 is structurally invalid.
        if value == 0x0FFF_FFF7 {
            return Err(Fat32Error::BadChain);
        }
        if value >= 0x0FFF_FFF8 {
            return Ok(value);
        }
        if value < 2 {
            return Err(Fat32Error::BadChain);
        }

        Ok(value)
    }
}

/// Helper to convert a file name to the 11-byte space-padded uppercase 8.3 form.
pub fn normalize_name(name: &str) -> Option<[u8; 11]> {
    let mut result = [b' '; 11];

    // Step 1: Split name into base and extension
    // We expect at most one dot separating the base name and extension.
    let mut parts = name.split('.');
    let base = parts.next()?;
    let ext = parts.next().unwrap_or("");

    // Step 2: Validate lengths
    // FAT 8.3 format restricts the base name to 8 characters and extension to 3 characters.
    // If the input exceeds this, or has multiple extensions, we reject it.
    if base.len() > 8 || ext.len() > 3 || parts.next().is_some() {
        return None;
    }

    // Step 3: Copy and convert to uppercase
    // We overwrite the space-padded array with the uppercase bytes from the base
    // and extension at their respective fixed offsets.
    for (i, b) in base.bytes().enumerate() {
        result[i] = b.to_ascii_uppercase();
    }
    for (i, b) in ext.bytes().enumerate() {
        result[8 + i] = b.to_ascii_uppercase();
    }

    Some(result)
}
