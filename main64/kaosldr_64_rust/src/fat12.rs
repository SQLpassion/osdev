use crate::ata::read_sectors;

const EOF_MARK: u16 = 0x0FF0;

// Memory buffer locations defined in fat12.c
pub const ROOT_DIR_BUFFER: *mut u8 = 0x30000 as *mut u8;
pub const FAT_BUFFER: *mut u8 = 0x31C00 as *mut u8;
pub const KERNEL_BUFFER: *mut u8 = 0xFFFF_8000_0010_0000 as *mut u8;

/// Represents a FAT12 Root Directory Entry.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct RootDirectoryEntry {
    pub file_name: [u8; 8],
    pub extension: [u8; 3],
    pub attributes: [u8; 1],
    pub reserved: [u8; 2],
    pub creation_time: [u8; 2],
    pub creation_date: [u8; 2],
    pub last_access_date: [u8; 2],
    pub ignore: [u8; 2],
    pub last_write_time: [u8; 2],
    pub last_write_date: [u8; 2],
    pub first_cluster: u16,
    pub file_size: u32,
}

/// A simple strcmp implementation comparing exactly len characters.
fn strcmp(s1: &[u8], s2: &[u8], len: usize) -> bool {
    if s1.len() < len || s2.len() < len {
        return false;
    }
    for i in 0..len {
        if s1[i] != s2[i] {
            return false;
        }
    }
    true
}

/// Finds a given Root Directory Entry by its Filename.
///
/// # Safety
/// The caller must ensure that the Root Directory has been loaded into `ROOT_DIR_BUFFER`.
unsafe fn find_root_directory_entry(filename: &[u8; 11]) -> Option<&'static RootDirectoryEntry> {
    // 16 entries fit in a sector, the directory size in FAT12 is 14 sectors * 16 entries/sector = 224 entries.
    // In C, it only loops 16 times!
    // Wait, let's look at C:
    // `for (i = 0; i < 16; i++) { ... }`
    // Yes, the C code only searched the first 16 entries of the loaded root directory.
    // To match the behavior of the C code, we will search 16 entries.
    let entries = core::slice::from_raw_parts(ROOT_DIR_BUFFER as *const RootDirectoryEntry, 16);

    for entry in entries {
        if entry.file_name[0] != 0x00 {
            // Concatenate the name and extension to compare with filename
            let mut full_name = [0u8; 11];
            full_name[..8].copy_from_slice(&entry.file_name);
            full_name[8..].copy_from_slice(&entry.extension);

            if strcmp(&full_name, filename, 11) {
                return Some(entry);
            }
        }
    }
    None
}

/// Load all Clusters for the given Root Directory Entry into memory.
///
/// # Safety
/// The caller must ensure that `kernelBuffer` is writable and that the FAT table has been loaded.
unsafe fn load_file_into_memory(entry: &RootDirectoryEntry) -> i32 {
    let mut sector_count = 0;
    let mut current_kernel_buffer = KERNEL_BUFFER;

    // Read the first cluster of the file into memory.
    // Sector LBA = cluster + 33 - 2
    read_sectors(current_kernel_buffer, (entry.first_cluster as u32) + 33 - 2, 1);
    sector_count += 1;

    let mut next_cluster = fat_read(entry.first_cluster);

    // Read the whole file into memory until we reach the EOF mark.
    while next_cluster < EOF_MARK {
        current_kernel_buffer = current_kernel_buffer.add(512);
        read_sectors(current_kernel_buffer, (next_cluster as u32) + 33 - 2, 1);
        sector_count += 1;

        // Read the next Cluster from the FAT table.
        next_cluster = fat_read(next_cluster);
    }

    sector_count
}

/// Reads the next FAT Entry from the FAT Tables.
///
/// # Safety
/// The caller must ensure that the FAT table has been loaded into `FAT_BUFFER`.
unsafe fn fat_read(cluster: u16) -> u16 {
    // Calculate the offset into the FAT table: fatOffset = (Cluster / 2) + Cluster
    let fat_offset = ((cluster / 2) + cluster) as usize;

    // Read the entry from the FAT
    let val_low = *FAT_BUFFER.add(fat_offset) as u16;
    let val_high = *FAT_BUFFER.add(fat_offset + 1) as u16;
    let val = val_low | (val_high << 8);

    if (cluster & 0x0001) != 0 {
        // Odd Cluster: Highest 12 Bits
        val >> 4
    } else {
        // Even Cluster: Lowest 12 Bits
        val & 0x0FFF
    }
}

/// Loads the given Kernel file into memory. Returns the size of the kernel in sectors.
///
/// # Safety
/// The caller must ensure the destination memory at KERNEL_BUFFER is writable,
/// and that the floppy disk controller is ready for operations.
pub unsafe fn load_kernel_into_memory(filename: &[u8; 11]) -> Result<i32, &'static str> {
    // Load the whole Root Directory (14 sectors starting at LBA 19) into memory.
    read_sectors(ROOT_DIR_BUFFER, 19, 14);

    // Find the Root Directory Entry for the kernel.
    if let Some(entry) = find_root_directory_entry(filename) {
        // Load the whole FAT (18 sectors starting at LBA 1) into memory.
        read_sectors(FAT_BUFFER, 1, 18);

        // Load the Kernel into memory.
        Ok(load_file_into_memory(entry))
    } else {
        Err("Kernel file not found")
    }
}
