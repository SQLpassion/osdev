//! File descriptor operations for FAT12.

use crate::drivers::block;
use crate::io::fat12::cluster::{
    allocate_new_cluster, deallocate_cluster_chain, fat12_next_cluster,
};
use crate::io::fat12::directory::{
    create_directory_entry, find_free_directory_slot, normalize_8_3_name, update_file_entry,
};
use crate::io::fat12::disk::{
    cluster_to_lba, read_fat_from_disk, read_root_directory_from_disk, write_fat_to_disk,
    write_root_directory_to_disk,
};
use crate::io::fat12::types::{
    EntryState, Fat12Error, FileDescriptor, FileMode, RawRootDirectoryEntry, ATTR_DIRECTORY,
    BYTES_PER_SECTOR, DIRECTORY_ENTRY_SIZE, FAT12_EOF_MIN, FAT12_MIN_DATA_CLUSTER,
    ROOT_DIRECTORY_ENTRIES,
};
use crate::sync::spinlock::SpinLock;
use alloc::vec::Vec;

pub static FILE_DESCRIPTORS: SpinLock<Vec<FileDescriptor>> = SpinLock::new(Vec::new());

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
            },
        };

        match entry.state() {
            EntryState::End => break,
            EntryState::Skip => continue,
            EntryState::Active => {
                if entry.short_name_raw() == normalized_name {
                    entry_index = Some((
                        entry_idx,
                        entry.first_cluster(),
                        entry.file_size(),
                        entry.attributes(),
                    ));
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
            let (idx, start_cluster, size) =
                if let Some((idx, first_cluster, size, _)) = entry_index {
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
    let entry = fds
        .iter_mut()
        .find(|e| e.fd == fd)
        .ok_or(Fat12Error::NotFound)?;

    if entry.current_offset >= entry.file_size {
        return Ok(0);
    }

    let bytes_to_read = core::cmp::min(
        buffer.len(),
        (entry.file_size - entry.current_offset) as usize,
    );
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
        block::read_sectors(cluster_lba as u64, 1, &mut temp_buffer)?;

        let chunk_offset = if bytes_read == 0 { byte_offset } else { 0 };
        let chunk_len = core::cmp::min(bytes_to_read - bytes_read, BYTES_PER_SECTOR - chunk_offset);

        buffer[bytes_read..bytes_read + chunk_len]
            .copy_from_slice(&temp_buffer[chunk_offset..chunk_offset + chunk_len]);

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
    let entry = fds
        .iter_mut()
        .find(|e| e.fd == fd)
        .ok_or(Fat12Error::NotFound)?;

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
        let chunk_len = core::cmp::min(
            bytes_to_write - bytes_written,
            BYTES_PER_SECTOR - chunk_offset,
        );

        if chunk_len < BYTES_PER_SECTOR {
            block::read_sectors(cluster_lba as u64, 1, &mut temp_buffer)?;
        }

        temp_buffer[chunk_offset..chunk_offset + chunk_len]
            .copy_from_slice(&buffer[bytes_written..bytes_written + chunk_len]);
        block::write_sectors(cluster_lba as u64, 1, &temp_buffer)?;

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

    update_file_entry(
        &mut root_dir,
        entry.root_entry_index,
        entry.file_size,
        entry.start_cluster,
    );
    write_root_directory_to_disk(&root_dir)?;
    write_fat_to_disk(&fat)?;

    Ok(bytes_written)
}
