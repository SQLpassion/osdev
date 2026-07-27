//! Filesystem-related system call implementations.

use crate::syscall::types::{
    is_valid_user_buffer_readable, is_valid_user_buffer_writable, SyscallError, SyscallResult,
};

/// Helper to read a null-terminated string from user space safely.
pub fn read_user_string(
    ptr: *const u8,
    max_len: usize,
) -> Result<alloc::string::String, SyscallError> {
    if ptr.is_null() {
        return Err(SyscallError::InvalidArg);
    }

    let mut len = 0;
    loop {
        if len >= max_len {
            return Err(SyscallError::InvalidArg);
        }

        // SAFETY:
        // - `len` starts at 0 and only ever grows past this point after the
        //   previous iteration's `ptr.add(len)` was validated by
        //   `is_valid_user_buffer_readable` and successfully dereferenced.
        // - The loop guard above (`len >= max_len`) bounds `len` to
        //   `max_len - 1` at this point, so `ptr.add(len)` cannot walk past
        //   the caller-supplied maximum length.
        // - This computes a pointer value only (no dereference); the result
        //   is validated for user-accessibility immediately below before it
        //   is ever read.
        if !is_valid_user_buffer_readable(unsafe { ptr.add(len) }, 1) {
            return Err(SyscallError::InvalidArg);
        }

        // SAFETY:
        // - We validated the pointer points to a valid user memory buffer.
        let b = unsafe { *ptr.add(len) };
        if b == 0 {
            break;
        }
        len += 1;
    }

    // SAFETY:
    // - We validated the memory range from ptr to ptr + len is valid user space.
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    let s = core::str::from_utf8(slice).map_err(|_| SyscallError::InvalidArg)?;
    Ok(alloc::string::String::from(s))
}

/// Implements `OpenFile()`.
pub fn syscall_open_file_impl(name_ptr: *const u8, mode_raw: u64) -> SyscallResult<u64> {
    let name = read_user_string(name_ptr, 128)?;
    let mode = match mode_raw {
        0 => crate::io::vfs::FileMode::Read,
        1 => crate::io::vfs::FileMode::Write,
        2 => crate::io::vfs::FileMode::Append,
        _ => return Err(SyscallError::InvalidArg),
    };

    let fd = crate::io::vfs::open(&name, mode).map_err(map_fs_error)?;
    Ok(fd as u64)
}

/// Implements `CloseFile()`.
pub fn syscall_close_file_impl(fd: u64) -> SyscallResult<u64> {
    crate::io::vfs::close(fd as usize).map_err(map_fs_error)?;
    Ok(0)
}

/// Implements `ReadFile()`.
pub fn syscall_read_file_impl(fd: u64, buf_ptr: *mut u8, len: u64) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }
    if !is_valid_user_buffer_writable(buf_ptr, len as usize) {
        return Err(SyscallError::InvalidArg);
    }

    // SAFETY:
    // - We validated every page in the buffer is present, user-accessible, and writable.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len as usize) };
    let bytes_read = crate::io::vfs::read(fd as usize, buffer).map_err(map_fs_error)?;
    Ok(bytes_read as u64)
}

/// Implements `WriteFile()`.
pub fn syscall_write_file_impl(fd: u64, buf_ptr: *const u8, len: u64) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }
    if !is_valid_user_buffer_readable(buf_ptr, len as usize) {
        return Err(SyscallError::InvalidArg);
    }

    // SAFETY:
    // - We validated the buffer is a valid user memory range.
    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, len as usize) };
    let bytes_written = crate::io::vfs::write(fd as usize, buffer).map_err(map_fs_error)?;
    Ok(bytes_written as u64)
}

/// Implements `DeleteFile()`.
pub fn syscall_delete_file_impl(name_ptr: *const u8) -> SyscallResult<u64> {
    let name = read_user_string(name_ptr, 128)?;
    crate::io::vfs::delete(&name).map_err(map_fs_error)?;
    Ok(0)
}

/// Implements `SeekFile()`.
pub fn syscall_seek_file_impl(fd: u64, offset: u64) -> SyscallResult<u64> {
    // The VFS backend's `seek()` takes a `u32` offset. A silent `as u32`
    // truncation here would let a caller pass e.g. `0x1_0000_0000` and have
    // it quietly land on offset `0` instead - a wrong seek executed instead
    // of a rejected one. Reject anything that would not round-trip instead.
    if offset > u32::MAX as u64 {
        return Err(SyscallError::InvalidArg);
    }

    crate::io::vfs::seek(fd as usize, offset as u32).map_err(map_fs_error)?;
    Ok(0)
}

/// Implements `EndOfFile()`.
pub fn syscall_end_of_file_impl(fd: u64) -> SyscallResult<u64> {
    let eof = crate::io::vfs::eof(fd as usize).map_err(map_fs_error)?;
    Ok(if eof { 1 } else { 0 })
}

/// Implements `PrintRootDirectory()`.
pub fn syscall_print_root_directory_impl() -> SyscallResult<u64> {
    crate::io::vfs::print_root_directory();
    Ok(0)
}

fn map_fs_error(err: crate::io::vfs::FsError) -> SyscallError {
    match err {
        crate::io::vfs::FsError::InvalidFd => SyscallError::InvalidArg,
        _ => SyscallError::Io,
    }
}
