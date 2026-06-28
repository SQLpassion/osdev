//! VFS implementation for the FAT12 filesystem.

use crate::io::fat12;
use crate::io::fat12::types::Fat12Error;
use crate::io::vfs::{FileMode, FileSystem, FsError};
use alloc::vec::Vec;

/// FileSystem adapter for the FAT12 filesystem implementation.
pub struct Fat12Fs;

impl FileSystem for Fat12Fs {
    fn open(&self, name: &str, mode: FileMode) -> Result<usize, FsError> {
        // Step 1: Forward the open call to fat12 and map errors without fd context.
        fat12::open_file(name, map_mode(mode)).map_err(|e| map_err(e, false))
    }

    fn close(&self, fd: usize) -> Result<(), FsError> {
        // Step 1: Forward the close call to fat12 and map errors with fd context.
        fat12::close_file(fd).map_err(|e| map_err(e, true))
    }

    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        // Step 1: Forward the read call to fat12 and map errors with fd context.
        fat12::read_file_fd(fd, buf).map_err(|e| map_err(e, true))
    }

    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize, FsError> {
        // Step 1: Forward the write call to fat12 and map errors with fd context.
        fat12::write_file_fd(fd, buf).map_err(|e| map_err(e, true))
    }

    fn seek(&self, fd: usize, offset: u32) -> Result<(), FsError> {
        // Step 1: Forward the seek call to fat12 and map errors with fd context.
        fat12::seek_file(fd, offset).map_err(|e| map_err(e, true))
    }

    fn eof(&self, fd: usize) -> Result<bool, FsError> {
        // Step 1: Forward the eof call to fat12 and map errors with fd context.
        fat12::eof_file(fd).map_err(|e| map_err(e, true))
    }

    fn delete(&self, name: &str) -> Result<(), FsError> {
        // Step 1: Forward the delete call to fat12 and map errors without fd context.
        fat12::delete_file(name).map_err(|e| map_err(e, false))
    }

    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError> {
        // Step 1: Forward the read_file call to fat12 and map errors without fd context.
        fat12::read_file(name).map_err(|e| map_err(e, false))
    }

    fn print_root_directory(&self) {
        // Step 1: Forward the print directory call to fat12.
        fat12::print_root_directory();
    }
}

/// Convert VFS FileMode to FAT12 FileMode.
fn map_mode(mode: FileMode) -> fat12::types::FileMode {
    match mode {
        FileMode::Read => fat12::types::FileMode::Read,
        FileMode::Write => fat12::types::FileMode::Write,
        FileMode::Append => fat12::types::FileMode::Append,
    }
}

/// Translate FAT12 error variants into VFS FsError variants.
///
/// Distinguishes between NotFound as FsError::NotFound (e.g. file missing)
/// or FsError::InvalidFd (e.g. fd missing in file descriptor table).
fn map_err(err: Fat12Error, fd_context: bool) -> FsError {
    match err {
        Fat12Error::NotFound => {
            if fd_context {
                FsError::InvalidFd
            } else {
                FsError::NotFound
            }
        }
        Fat12Error::InvalidFileName => FsError::InvalidName,
        Fat12Error::Block(crate::drivers::block::BlockError::Unsupported) => FsError::Unsupported,
        _ => FsError::Io,
    }
}
