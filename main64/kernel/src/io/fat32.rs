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
    /// An underlying block device read error occurred.
    Block(crate::drivers::block::BlockError),

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
        crate::drivers::block::read_sectors(part_lba, 1, &mut sector).map_err(Fat32Error::Block)?;

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
                crate::drivers::block::read_sectors(cluster_lba + i as u64, 1, &mut sector)
                    .map_err(Fat32Error::Block)?;

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
                crate::drivers::block::read_sectors(cluster_lba + i as u64, 1, &mut sector)
                    .map_err(Fat32Error::Block)?;

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

    /// Walk the root directory and print all active entries.
    pub fn print_root_directory(&self) {
        let mut current_cluster = self.root_cluster;
        let mut file_count = 0;
        let mut total_size = 0;

        crate::console::with_console(|console| {
            let mut cluster_count = 0;
            'dir_walk: while (2..0x0FFF_FFF8).contains(&current_cluster) {
                cluster_count += 1;
                if cluster_count > 1_000_000 {
                    break;
                }

                let cluster_lba = self.cluster_to_lba(current_cluster);
                for i in 0..self.sec_per_clus {
                    let mut sector = [0u8; 512];
                    if crate::drivers::block::read_sectors(cluster_lba + i as u64, 1, &mut sector)
                        .is_err()
                    {
                        let _ = writeln!(console, "FAT32 read error during print_root_directory");
                        return;
                    }

                    for entry_idx in 0..(512 / 32) {
                        let offset = entry_idx * 32;
                        let first_byte = sector[offset];
                        if first_byte == 0x00 {
                            break 'dir_walk;
                        }
                        if first_byte == 0xE5 {
                            continue;
                        }
                        let attr = sector[offset + 0x0B];
                        if attr == 0x0F {
                            continue;
                        }

                        // Parse the name in standard 8.3 format
                        let mut name_buf = [0u8; 13];
                        let mut pos = 0;

                        // Base name (8 bytes)
                        for &b in &sector[offset..offset + 8] {
                            if b == b' ' {
                                break;
                            }
                            name_buf[pos] = b.to_ascii_lowercase();
                            pos += 1;
                        }

                        // Extension (3 bytes)
                        let ext = &sector[offset + 8..offset + 11];
                        if ext.iter().any(|&b| b != b' ') {
                            name_buf[pos] = b'.';
                            pos += 1;
                            for &b in ext {
                                if b == b' ' {
                                    break;
                                }
                                name_buf[pos] = b.to_ascii_lowercase();
                                pos += 1;
                            }
                        }

                        let name = core::str::from_utf8(&name_buf[..pos]).unwrap_or("???");

                        let clus_hi = u16::from_le_bytes(
                            sector[offset + 0x14..offset + 0x16].try_into().unwrap(),
                        ) as u32;
                        let clus_lo = u16::from_le_bytes(
                            sector[offset + 0x1A..offset + 0x1C].try_into().unwrap(),
                        ) as u32;
                        let first_cluster = (clus_hi << 16) | clus_lo;
                        let file_size = u32::from_le_bytes(
                            sector[offset + 0x1C..offset + 0x20].try_into().unwrap(),
                        );

                        // Format and print each directory entry's size, start cluster, and name
                        let _ = write!(console, "{} bytes", file_size);
                        let _ = write!(console, "\tStart Cluster: {}", first_cluster);
                        let _ = write!(console, "\t{}", name);
                        console.print_char(b'\n');

                        // Summary metrics only for regular files
                        let is_directory = (attr & 0x10) != 0;
                        if !is_directory {
                            file_count += 1;
                            total_size += file_size;
                        }
                    }
                }

                // Look up the next cluster
                if let Ok(next) = self.next_cluster(current_cluster) {
                    current_cluster = next;
                } else {
                    break;
                }
            }

            // Print footer
            let _ = write!(console, "\t\t{} File(s)", file_count);
            let _ = write!(console, "\t{} bytes", total_size);
            console.print_char(b'\n');
        });
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
        crate::drivers::block::read_sectors(fat_sector, 1, &mut sector)
            .map_err(Fat32Error::Block)?;

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

// ---- FAT32 Open File state cache --------------------------------------------

struct Fat32OpenFile {
    #[allow(dead_code)]
    name: alloc::string::String,
    data: Vec<u8>,
    offset: usize,
}

// ---- FAT32 VFS adapter ------------------------------------------------------

/// FileSystem adapter for the FAT32 volume implementation.
pub struct Fat32Fs {
    volume: Fat32Volume,
    open_files: crate::sync::spinlock::SpinLock<alloc::vec::Vec<Option<Fat32OpenFile>>>,
}

impl Fat32Fs {
    pub fn new(volume: Fat32Volume) -> Self {
        Self {
            volume,
            open_files: crate::sync::spinlock::SpinLock::new(alloc::vec::Vec::new()),
        }
    }
}

impl crate::io::vfs::FileSystem for Fat32Fs {
    fn open(
        &self,
        name: &str,
        mode: crate::io::vfs::FileMode,
    ) -> Result<usize, crate::io::vfs::FsError> {
        // Step 1: Reject any write operations since FAT32 is currently read-only.
        if mode != crate::io::vfs::FileMode::Read {
            return Err(crate::io::vfs::FsError::Unsupported);
        }

        // Step 2: Read whole file content without holding the lock (prevent deadlock/interrupt-disabling during block I/O).
        let data = self.volume.read_file(name).map_err(map_fat32_err)?;

        // Step 3: Lock the open file descriptors table and insert the newly opened file.
        let mut files = self.open_files.lock();
        let file = Fat32OpenFile {
            name: alloc::string::String::from(name),
            data,
            offset: 0,
        };

        if let Some(free_idx) = files.iter().position(|slot| slot.is_none()) {
            files[free_idx] = Some(file);
            Ok(free_idx)
        } else {
            files.push(Some(file));
            Ok(files.len() - 1)
        }
    }

    fn close(&self, fd: usize) -> Result<(), crate::io::vfs::FsError> {
        // Step 1: Lock files list and remove active descriptor at the index.
        let mut files = self.open_files.lock();
        if fd >= files.len() || files[fd].is_none() {
            return Err(crate::io::vfs::FsError::InvalidFd);
        }
        files[fd] = None;
        Ok(())
    }

    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize, crate::io::vfs::FsError> {
        // Step 1: Lock files list and retrieve a mutable reference to the open file state.
        let mut files = self.open_files.lock();
        if fd >= files.len() {
            return Err(crate::io::vfs::FsError::InvalidFd);
        }
        let file = files[fd]
            .as_mut()
            .ok_or(crate::io::vfs::FsError::InvalidFd)?;

        // Step 2: Return 0 immediately if the cursor is already at EOF.
        if file.offset >= file.data.len() {
            return Ok(0);
        }

        // Step 3: Copy bytes from cache to output buffer and advance cursor.
        let bytes_to_read = core::cmp::min(buf.len(), file.data.len() - file.offset);
        buf[..bytes_to_read].copy_from_slice(&file.data[file.offset..file.offset + bytes_to_read]);
        file.offset += bytes_to_read;
        Ok(bytes_to_read)
    }

    fn write(&self, _fd: usize, _buf: &[u8]) -> Result<usize, crate::io::vfs::FsError> {
        // Out of scope: FAT32 is read-only.
        Err(crate::io::vfs::FsError::Unsupported)
    }

    fn seek(&self, fd: usize, offset: u32) -> Result<(), crate::io::vfs::FsError> {
        // Step 1: Lock files list and retrieve file state.
        let mut files = self.open_files.lock();
        if fd >= files.len() {
            return Err(crate::io::vfs::FsError::InvalidFd);
        }
        let file = files[fd]
            .as_mut()
            .ok_or(crate::io::vfs::FsError::InvalidFd)?;

        // Step 2: Ensure offset doesn't exceed file size (mirroring FAT12's UnexpectedEof behavior).
        if offset as usize > file.data.len() {
            return Err(crate::io::vfs::FsError::Io);
        }
        file.offset = offset as usize;
        Ok(())
    }

    fn eof(&self, fd: usize) -> Result<bool, crate::io::vfs::FsError> {
        // Step 1: Lock files list and verify cursor offset is at or past size.
        let files = self.open_files.lock();
        if fd >= files.len() {
            return Err(crate::io::vfs::FsError::InvalidFd);
        }
        let file = files[fd]
            .as_ref()
            .ok_or(crate::io::vfs::FsError::InvalidFd)?;
        Ok(file.offset >= file.data.len())
    }

    fn delete(&self, _name: &str) -> Result<(), crate::io::vfs::FsError> {
        // Out of scope: FAT32 is read-only.
        Err(crate::io::vfs::FsError::Unsupported)
    }

    fn read_file(&self, name: &str) -> Result<alloc::vec::Vec<u8>, crate::io::vfs::FsError> {
        // Step 1: Directly forward to volume read_file without lock.
        self.volume.read_file(name).map_err(map_fat32_err)
    }

    fn print_root_directory(&self) {
        // Step 1: Directly forward to volume print_root_directory.
        self.volume.print_root_directory();
    }
}

/// Translate FAT32 errors into VFS FsError variants.
fn map_fat32_err(err: Fat32Error) -> crate::io::vfs::FsError {
    match err {
        Fat32Error::NotFound => crate::io::vfs::FsError::NotFound,
        Fat32Error::Block(crate::drivers::block::BlockError::Unsupported) => {
            crate::io::vfs::FsError::Unsupported
        }
        _ => crate::io::vfs::FsError::Io,
    }
}
