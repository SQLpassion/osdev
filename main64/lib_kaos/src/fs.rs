//! File-system syscall wrappers: open, read, write, delete, and directory listing.

use crate::{
    raw::{syscall0, syscall1, syscall2, syscall3},
    SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_IO,
};

/// Open mode passed to [`File::open`].
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Read   = 0,
    Write  = 1,
    Append = 2,
}

// ── raw helpers ──────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn raw_open_file(name_ptr: *const u8, mode: FileMode) -> u64 {
    // SAFETY: Caller provides a valid pointer to a null-terminated filename.
    unsafe { syscall2(SyscallId::OpenFile as u64, name_ptr as u64, mode as u64) }
}

#[inline(always)]
unsafe fn raw_close_file(fd: u64) -> u64 {
    // SAFETY: Caller provides a valid open file descriptor.
    unsafe { syscall1(SyscallId::CloseFile as u64, fd) }
}

#[inline(always)]
unsafe fn raw_read_file(fd: u64, buf_ptr: *mut u8, len: usize) -> u64 {
    // SAFETY: Caller guarantees `buf_ptr` is valid for writing `len` bytes.
    unsafe { syscall3(SyscallId::ReadFile as u64, fd, buf_ptr as u64, len as u64) }
}

#[inline(always)]
unsafe fn raw_write_file(fd: u64, buf_ptr: *const u8, len: usize) -> u64 {
    // SAFETY: Caller guarantees `buf_ptr` is valid for reading `len` bytes.
    unsafe { syscall3(SyscallId::WriteFile as u64, fd, buf_ptr as u64, len as u64) }
}

#[inline(always)]
unsafe fn raw_delete_file(name_ptr: *const u8) -> u64 {
    // SAFETY: Caller provides a valid pointer to a null-terminated filename.
    unsafe { syscall1(SyscallId::DeleteFile as u64, name_ptr as u64) }
}

// ── public API ───────────────────────────────────────────────────────────────

/// RAII handle to an open file descriptor.
///
/// Automatically closes the descriptor when dropped.
pub struct File {
    fd: u64,
}

impl File {
    /// Opens `name` with the given `mode`.
    ///
    /// `name` is null-terminated in a stack buffer before the syscall.
    pub fn open(name: &[u8], mode: FileMode) -> Result<Self, u64> {
        let mut buf = [0u8; 64];
        if name.len() >= 64 {
            return Err(SYSCALL_ERR_INVALID_ARG);
        }
        buf[..name.len()].copy_from_slice(name);
        buf[name.len()] = 0;

        let fd = unsafe {
            // SAFETY: `buf` is a valid null-terminated string on the stack.
            raw_open_file(buf.as_ptr(), mode)
        };
        if fd >= SYSCALL_ERR_IO {
            return Err(fd);
        }
        Ok(Self { fd })
    }

    /// Reads up to `buf.len()` bytes from the file.
    ///
    /// Returns the number of bytes actually read (`0` on EOF).
    #[allow(clippy::cast_possible_truncation)]
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, u64> {
        let raw = unsafe {
            // SAFETY: `buf` is a valid slice in user memory.
            raw_read_file(self.fd, buf.as_mut_ptr(), buf.len())
        };
        if raw >= SYSCALL_ERR_IO {
            return Err(raw);
        }
        Ok(raw as usize)
    }

    /// Writes `buf` to the file.
    ///
    /// Returns the number of bytes actually written.
    #[allow(clippy::cast_possible_truncation)]
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, u64> {
        let raw = unsafe {
            // SAFETY: `buf` is a valid slice in user memory.
            raw_write_file(self.fd, buf.as_ptr(), buf.len())
        };
        if raw >= SYSCALL_ERR_IO {
            return Err(raw);
        }
        Ok(raw as usize)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: `self.fd` is a valid open descriptor obtained in `open`.
            let _ = raw_close_file(self.fd);
        }
    }
}

/// Returns `true` when `name` exists on disk.
///
/// Attempts a read-only open; the resulting `File` is immediately dropped.
pub fn file_exists(name: &[u8]) -> bool {
    File::open(name, FileMode::Read).is_ok()
}

/// Deletes the file named `name`.
///
/// `name` is null-terminated in a stack buffer before the syscall.
pub fn delete_file(name: &[u8]) -> Result<(), u64> {
    let mut buf = [0u8; 64];
    if name.len() >= 64 {
        return Err(SYSCALL_ERR_INVALID_ARG);
    }
    buf[..name.len()].copy_from_slice(name);
    buf[name.len()] = 0;

    let raw = unsafe {
        // SAFETY: `buf` is a valid null-terminated string on the stack.
        raw_delete_file(buf.as_ptr())
    };
    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }
    Ok(())
}

/// Prints the root directory listing of the FAT12 disk to the console.
#[inline(always)]
pub fn print_root_directory() -> Result<(), u64> {
    let raw = unsafe {
        // SAFETY: `PrintRootDirectory` takes no pointer arguments.
        syscall0(SyscallId::PrintRootDirectory as u64)
    };
    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }
    Ok(())
}
