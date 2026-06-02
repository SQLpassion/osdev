//! Minimal user-mode syscall ABI wrappers (`int 0x80`).

#![allow(dead_code)]

use core::arch::asm;

/// Stable syscall numbers exposed by the kernel.
#[repr(u64)]
#[allow(dead_code)]
enum SyscallId {
    Exit = 2,
    WriteConsole = 3,
    GetChar = 4,
    OpenFile = 8,
    CloseFile = 9,
    ReadFile = 10,
    WriteFile = 11,
    DeleteFile = 12,
}

#[allow(dead_code)]
const SYSCALL_ERR_UNSUPPORTED: u64 = u64::MAX;
#[allow(dead_code)]
const SYSCALL_ERR_INVALID_ARG: u64 = u64::MAX - 1;
#[allow(dead_code)]
const SYSCALL_ERR_IO: u64 = u64::MAX - 2;

#[inline(always)]
unsafe fn syscall0(syscall_nr: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - Caller ensures this code runs in a context where `int 0x80` is valid.
    // - Register assignment matches kernel syscall ABI.
    unsafe {
        asm!(
            "int 0x80",
            inout("rax") ret,
            in("rdi") 0u64,
            in("rsi") 0u64,
            in("rdx") 0u64,
            in("r10") 0u64
        );
    }

    ret
}

#[inline(always)]
unsafe fn syscall1(syscall_nr: u64, arg0: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - Caller ensures this code runs in a context where `int 0x80` is valid.
    // - Register assignment matches kernel syscall ABI.
    unsafe {
        asm!(
            "int 0x80",
            inout("rax") ret,
            in("rdi") arg0,
            in("rsi") 0u64,
            in("rdx") 0u64,
            in("r10") 0u64
        );
    }

    ret
}

#[inline(always)]
unsafe fn syscall2(syscall_nr: u64, arg0: u64, arg1: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - Caller ensures this code runs in a context where `int 0x80` is valid.
    // - Register assignment matches kernel syscall ABI.
    unsafe {
        asm!(
            "int 0x80",
            inout("rax") ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") 0u64,
            in("r10") 0u64
        );
    }

    ret
}

#[inline(always)]
unsafe fn syscall3(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - Caller ensures this code runs in a context where `int 0x80` is valid.
    // - Register assignment matches kernel syscall ABI.
    unsafe {
        asm!(
            "int 0x80",
            inout("rax") ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") 0u64
        );
    }

    ret
}

/// Open mode for user programs.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileMode {
    Read = 0,
    Write = 1,
    Append = 2,
}

/// Opens a file. Returns file descriptor ID or raw error sentinel.
#[inline(always)]
unsafe fn raw_open_file(name_ptr: *const u8, mode: FileMode) -> u64 {
    // SAFETY:
    // - Caller must provide a valid pointer to a null-terminated string.
    unsafe { syscall2(SyscallId::OpenFile as u64, name_ptr as u64, mode as u64) }
}

/// Closes a file descriptor.
#[inline(always)]
unsafe fn raw_close_file(fd: u64) -> u64 {
    unsafe { syscall1(SyscallId::CloseFile as u64, fd) }
}

/// Reads data from a file descriptor into user memory.
#[inline(always)]
unsafe fn raw_read_file(fd: u64, buf_ptr: *mut u8, len: usize) -> u64 {
    // SAFETY:
    // - Caller must guarantee buffer pointer is valid for writing len bytes.
    unsafe { syscall3(SyscallId::ReadFile as u64, fd, buf_ptr as u64, len as u64) }
}

/// Writes data from user memory into a file descriptor.
#[inline(always)]
unsafe fn raw_write_file(fd: u64, buf_ptr: *const u8, len: usize) -> u64 {
    // SAFETY:
    // - Caller must guarantee buffer pointer is valid for reading len bytes.
    unsafe { syscall3(SyscallId::WriteFile as u64, fd, buf_ptr as u64, len as u64) }
}

/// Deletes a file.
#[inline(always)]
unsafe fn raw_delete_file(name_ptr: *const u8) -> u64 {
    // SAFETY:
    // - Caller must provide a valid pointer to a null-terminated string.
    unsafe { syscall1(SyscallId::DeleteFile as u64, name_ptr as u64) }
}

/// Writes bytes to VGA console via kernel syscall.
#[inline(always)]
unsafe fn raw_write_console(ptr: *const u8, len: usize) -> u64 {
    // SAFETY:
    // - Caller provides buffer pointer/length according to syscall contract.
    unsafe { syscall2(SyscallId::WriteConsole as u64, ptr as u64, len as u64) }
}

/// Safe wrapper for writing a slice of bytes to the console.
#[inline(always)]
pub fn write_console(msg: &[u8]) -> Result<(), u64> {
    let raw = unsafe {
        // SAFETY:
        // - msg is a valid slice in memory.
        raw_write_console(msg.as_ptr(), msg.len())
    };
    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }
    Ok(())
}

/// Safe wrapper for deleting a file.
/// Automatically handles null-termination via a stack buffer.
#[inline(always)]
pub fn delete_file(name: &[u8]) -> Result<(), u64> {
    let mut buf = [0u8; 64];
    if name.len() >= 64 {
        return Err(SYSCALL_ERR_INVALID_ARG);
    }
    buf[..name.len()].copy_from_slice(name);
    buf[name.len()] = 0;

    let raw = unsafe {
        // SAFETY:
        // - buf is a valid null-terminated string on the stack.
        raw_delete_file(buf.as_ptr())
    };
    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }
    Ok(())
}

/// Safe wrapper for an open file descriptor.
/// Automatically closes the file when it goes out of scope (RAII).
pub struct File {
    fd: u64,
}

impl File {
    /// Opens a file with the given name and mode.
    /// Handles null-termination safely via a stack buffer.
    pub fn open(name: &[u8], mode: FileMode) -> Result<Self, u64> {
        let mut buf = [0u8; 64];
        if name.len() >= 64 {
            return Err(SYSCALL_ERR_INVALID_ARG);
        }
        buf[..name.len()].copy_from_slice(name);
        buf[name.len()] = 0;

        let fd = unsafe {
            // SAFETY:
            // - buf is a valid null-terminated string on the stack.
            raw_open_file(buf.as_ptr(), mode)
        };
        if fd >= SYSCALL_ERR_IO {
            return Err(fd);
        }
        Ok(Self { fd })
    }

    /// Reads data from the file into the provided buffer.
    /// Returns the number of bytes read.
    #[allow(clippy::cast_possible_truncation)]
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, u64> {
        let raw = unsafe {
            // SAFETY:
            // - buf is a valid slice in memory.
            raw_read_file(self.fd, buf.as_mut_ptr(), buf.len())
        };
        if raw >= SYSCALL_ERR_IO {
            return Err(raw);
        }
        Ok(raw as usize)
    }

    /// Writes data from the buffer to the file.
    /// Returns the number of bytes written.
    #[allow(clippy::cast_possible_truncation)]
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, u64> {
        let raw = unsafe {
            // SAFETY:
            // - buf is a valid slice in memory.
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
            // SAFETY:
            // - self.fd is a valid open file descriptor to close.
            let _ = raw_close_file(self.fd);
        }
    }
}

/// Checks if a file exists on the disk.
/// The temporary File object is immediately closed on return.
pub fn file_exists(name: &[u8]) -> bool {
    File::open(name, FileMode::Read).is_ok()
}

/// Reads one decoded keyboard character via kernel syscall.
#[inline(always)]
#[allow(dead_code)]
#[allow(clippy::cast_possible_truncation)]
fn getchar() -> Result<u8, u64> {
    let raw = unsafe {
        // SAFETY:
        // - GetChar uses no pointer arguments.
        syscall0(SyscallId::GetChar as u64)
    };

    if raw == SYSCALL_ERR_UNSUPPORTED || raw == SYSCALL_ERR_INVALID_ARG || raw == SYSCALL_ERR_IO {
        return Err(raw);
    }

    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }

    Ok(raw as u8)
}

/// Reads one line from keyboard and echoes input via `WriteConsole`.
///
/// Newline is echoed but not written into `buf`.
#[inline(always)]
#[allow(dead_code)]
pub fn user_readline(buf: &mut [u8]) -> Result<usize, u64> {
    let mut len = 0usize;

    loop {
        let ch = getchar()?;

        match ch {
            b'\r' | b'\n' => {
                let newline = b'\n';
                let raw = unsafe {
                    // SAFETY:
                    // - `newline` is a valid local byte.
                    raw_write_console(&raw const newline, 1)
                };
                if raw >= SYSCALL_ERR_IO {
                    return Err(raw);
                }
                break;
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    let backspace = 0x08u8;
                    let raw = unsafe {
                        // SAFETY:
                        // - `backspace` is a valid local byte.
                        raw_write_console(&raw const backspace, 1)
                    };
                    if raw >= SYSCALL_ERR_IO {
                        return Err(raw);
                    }
                }
            }
            _ => {
                if len < buf.len() {
                    buf[len] = ch;
                    len += 1;
                    let raw = unsafe {
                        // SAFETY:
                        // - `ch` is a valid local byte.
                        raw_write_console(&raw const ch, 1)
                    };
                    if raw >= SYSCALL_ERR_IO {
                        return Err(raw);
                    }
                }
            }
        }
    }

    Ok(len)
}

/// Terminates the current user task.
#[inline(always)]
pub fn exit() -> ! {
    // SAFETY:
    // - Exit syscall is available through `int 0x80` ABI.
    let _ = unsafe { syscall0(SyscallId::Exit as u64) };

    loop {
        core::hint::spin_loop();
    }
}
