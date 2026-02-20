//! User-space oriented syscall wrappers.
//!
//! This module provides ergonomic wrappers around the raw `int 0x80` ABI:
//! - `sys_yield` for cooperative scheduling,
//! - `sys_write_serial` for debug output,
//! - `sys_write_console` for VGA text output,
//! - `sys_getchar` for blocking keyboard input,
//! - `sys_exit` to terminate the current task.
//!
//! Design goals:
//! - keep call sites simple (`Result`-based API where possible),
//! - keep syscall return decoding explicit at wrapper boundaries,
//! - keep unsafe and ABI details local to wrapper implementations.

use core::arch::asm;

use super::{
    abi, SysError, SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_IO, SYSCALL_ERR_UNSUPPORTED,
};

/// Normalizes a raw input byte for one-shot console echo.
///
/// The keyboard path may deliver carriage return (`'\r'`) for Enter.
/// VGA-style line handling in this kernel is newline-centric (`'\n'`),
/// so Enter is normalized to a single newline byte to avoid double
/// line transitions in user-space echo loops.
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub const fn normalize_echo_input_byte(ch: u8) -> u8 {
    if ch == b'\r' {
        b'\n'
    } else {
        ch
    }
}

/// Requests a cooperative reschedule.
///
/// This invokes syscall `Yield` and returns `Ok(())` on success.
/// Any kernel error sentinel is translated to `SysError`.
#[inline(always)]
#[allow(dead_code)]
pub fn sys_yield() -> Result<(), SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - This requires `unsafe` because syscall entry executes raw ABI-level interrupt machinery.
        // - Wrapper is intended for ring-3/ring-0 contexts where `int 0x80` is configured.
        abi::syscall0(SyscallId::Yield as u64)
    };

    // Keep decoding local in this module:
    // - User wrappers may execute in ring-3 via aliased code pages.
    // - Calling an external decode helper can jump to an unmapped
    //   higher-half kernel text page and fault in user mode.
    // - This explicit match keeps the wrapper path self-contained.
    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        x if x >= SYSCALL_ERR_INVALID_ARG => Err(SysError::Unknown(x)),
        _ => Ok(()),
    }
}

/// Writes `len` bytes from `ptr` to the kernel debug serial output (COM1).
///
/// ABI arguments:
/// - `arg0` (`RDI`) = `ptr`
/// - `arg1` (`RSI`) = `len`
///
/// Return value:
/// - `Ok(written)` with number of written bytes,
/// - `Err(...)` when kernel reports a syscall error.
///
/// # Safety
/// - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
/// Caller must ensure that `ptr..ptr+len` is readable in the current
/// user/kernel context expected by the syscall boundary.
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub unsafe fn sys_write_serial(ptr: *const u8, len: usize) -> Result<usize, SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - Caller guarantees `ptr`/`len` satisfy the required memory contract.
        abi::syscall2(SyscallId::WriteSerial as u64, ptr as u64, len as u64)
    };

    // Keep decoding local for the same reason as `sys_yield`: avoid extra
    // helper calls outside the user-aliased wrapper path.
    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        written => Ok(written as usize),
    }
}

/// Writes `len` bytes from `ptr` to the VGA text console.
///
/// ABI arguments:
/// - `arg0` (`RDI`) = `ptr`
/// - `arg1` (`RSI`) = `len`
///
/// Return value:
/// - `Ok(written)` with number of written bytes,
/// - `Err(...)` when kernel reports a syscall error.
///
/// # Safety
/// - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
/// Caller must ensure that `ptr..ptr+len` is readable in the current
/// user/kernel context expected by the syscall boundary.
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub unsafe fn sys_write_console(ptr: *const u8, len: usize) -> Result<usize, SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - Caller guarantees `ptr`/`len` satisfy the required memory contract.
        abi::syscall2(SyscallId::WriteConsole as u64, ptr as u64, len as u64)
    };

    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        written => Ok(written as usize),
    }
}

/// Reads a single character from the keyboard (blocking).
///
/// This syscall blocks the calling task until a character becomes available in
/// the keyboard input buffer. The keyboard driver's worker task decodes raw
/// scancodes into ASCII and wakes waiting tasks once input is ready.
///
/// # Blocking Behavior
/// If no input is available, the task will be put to sleep on the keyboard
/// wait queue and rescheduled automatically by the scheduler once the keyboard
/// worker task has decoded new input.
///
/// # Return Value
/// - `Ok(ch)` with the ASCII character code (0-255),
/// - `Err(...)` when kernel reports a syscall error (unlikely for this syscall).
///
/// # Examples
/// ```
/// // Read a character and echo it
/// if let Ok(ch) = sys_getchar() {
///     // Echo the character back to console
/// }
/// ```
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub fn sys_getchar() -> Result<u8, SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - This requires `unsafe` because syscall entry executes raw ABI-level interrupt machinery.
        // - Wrapper is intended for contexts where `int 0x80` is configured.
        // - GetChar has no memory arguments, so no buffer validation needed.
        abi::syscall0(SyscallId::GetChar as u64)
    };

    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        ch => Ok(ch as u8),
    }
}

/// Returns the current VGA cursor position as `(row, col)`.
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub fn sys_get_cursor() -> Result<(usize, usize), SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - This requires `unsafe` because syscall entry executes raw ABI-level interrupt machinery.
        // - Wrapper is intended for contexts where `int 0x80` is configured.
        // - GetCursor has no memory arguments.
        abi::syscall0(SyscallId::GetCursor as u64)
    };

    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        packed => Ok(((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize)),
    }
}

/// Sets the VGA cursor position.
///
/// Values outside bounds are clamped by the kernel screen driver.
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub fn sys_set_cursor(row: usize, col: usize) -> Result<(), SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - This requires `unsafe` because syscall entry executes raw ABI-level interrupt machinery.
        // - Wrapper is intended for contexts where `int 0x80` is configured.
        // - SetCursor takes plain integer arguments only.
        abi::syscall2(SyscallId::SetCursor as u64, row as u64, col as u64)
    };

    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        _ => Ok(()),
    }
}

/// Clears the VGA text screen and resets cursor to `(0, 0)`.
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub fn sys_clear_screen() -> Result<(), SysError> {
    let raw_value = unsafe {
        // SAFETY:
        // - This requires `unsafe` because syscall entry executes raw ABI-level interrupt machinery.
        // - Wrapper is intended for contexts where `int 0x80` is configured.
        // - ClearScreen takes no memory arguments.
        abi::syscall0(SyscallId::ClearScreen as u64)
    };

    match raw_value {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        _ => Ok(()),
    }
}

/// Terminates the current task.
///
/// Expected behavior:
/// - on a correct scheduler path, this never returns,
/// - if it unexpectedly returns, the function enters a local terminal loop.
///
/// # Exit Code
/// This syscall does not accept an exit code parameter. The kernel currently
/// does not track exit codes since there is no parent/child task relationship
/// or wait mechanism. If such functionality is added in the future, an exit
/// code parameter can be reintroduced.
#[inline(always)]
pub fn sys_exit() -> ! {
    let _ = unsafe {
        // SAFETY:
        // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
        // - Wrapper is intended for contexts where `int 0x80` exit syscall is available.
        abi::syscall0(SyscallId::Exit as u64)
    };
    unsafe {
        // SAFETY:
        // - Executed only as fallback if `sys_exit` unexpectedly returns.
        // - Keeps control in a local tight loop without calling external code.
        asm!("2:", "pause", "jmp 2b", options(noreturn));
    }
}

/// Reads a line of input into a buffer (user-space implementation).
///
/// This is a **user-space library function** built entirely on top of primitive
/// syscalls (`sys_getchar` and `sys_write_console`). It demonstrates the proper
/// separation between kernel mechanisms (syscalls) and user-space policy (line editing).
///
/// # Features
/// - Reads characters one by one using `sys_getchar()`
/// - Echoes characters to the console using `sys_write_console()`
/// - Handles backspace (0x08) with buffer management
/// - Stops on Enter (\\n or \\r)
/// - Newline is echoed but NOT stored in the buffer
///
/// # Parameters
/// - `buf`: Mutable buffer to store the input line
///
/// # Return Value
/// - `Ok(len)`: Number of bytes written to the buffer (excluding newline)
/// - `Err(...)`: Syscall error from `sys_getchar` or `sys_write_console`
///
/// # Example
/// ```
/// let mut buffer = [0u8; 128];
/// match user_readline(&mut buffer) {
///     Ok(len) => {
///         // Process buffer[..len]
///     }
///     Err(e) => {
///         // Handle error
///     }
/// }
/// ```
#[inline(always)]
#[cfg_attr(not(test), allow(dead_code))]
pub fn user_readline(buf: &mut [u8]) -> Result<usize, SysError> {
    let mut len = 0usize;

    loop {
        // Block until a character is available.
        // Keep decoding local to avoid pulling additional Result<T, E> helper
        // pages into the ring-3 alias mapping.
        let get_char_raw = unsafe {
            // SAFETY:
            // - This requires `unsafe` because syscall entry executes raw ABI-level interrupt machinery.
            // - Wrapper runs only where `int 0x80` syscall entry is configured.
            // - GetChar has no memory arguments.
            abi::syscall0(SyscallId::GetChar as u64)
        };
        if get_char_raw == SYSCALL_ERR_UNSUPPORTED {
            return Err(SysError::UnsupportedSyscall);
        }
        if get_char_raw == SYSCALL_ERR_INVALID_ARG {
            return Err(SysError::InvalidArgument);
        }
        if get_char_raw == SYSCALL_ERR_IO {
            return Err(SysError::IoError);
        }
        if get_char_raw >= SYSCALL_ERR_IO {
            return Err(SysError::Unknown(get_char_raw));
        }
        let ch = get_char_raw as u8;

        match ch {
            // Enter key - finish reading
            b'\r' | b'\n' => {
                // Echo newline - use stack variable to avoid .rodata reference
                let newline = b'\n';
                let raw = unsafe {
                    // SAFETY:
                    // - This requires `unsafe` because syscall writes use a raw pointer/length ABI.
                    // - `newline` is a valid stack byte and readable for 1 byte.
                    abi::syscall2(
                        SyscallId::WriteConsole as u64,
                        (&newline as *const u8) as u64,
                        1,
                    )
                };
                if raw == SYSCALL_ERR_UNSUPPORTED {
                    return Err(SysError::UnsupportedSyscall);
                }
                if raw == SYSCALL_ERR_INVALID_ARG {
                    return Err(SysError::InvalidArgument);
                }
                if raw == SYSCALL_ERR_IO {
                    return Err(SysError::IoError);
                }
                if raw >= SYSCALL_ERR_IO {
                    return Err(SysError::Unknown(raw));
                }
                break;
            }

            // Backspace - remove last character
            0x08 => {
                if len > 0 {
                    len -= 1;
                    // Echo backspace to move cursor back - use stack variable
                    let backspace = 0x08u8;
                    let raw = unsafe {
                        // SAFETY:
                        // - This requires `unsafe` because syscall writes use a raw pointer/length ABI.
                        // - `backspace` is a valid stack byte and readable for 1 byte.
                        abi::syscall2(
                            SyscallId::WriteConsole as u64,
                            (&backspace as *const u8) as u64,
                            1,
                        )
                    };
                    if raw == SYSCALL_ERR_UNSUPPORTED {
                        return Err(SysError::UnsupportedSyscall);
                    }
                    if raw == SYSCALL_ERR_INVALID_ARG {
                        return Err(SysError::InvalidArgument);
                    }
                    if raw == SYSCALL_ERR_IO {
                        return Err(SysError::IoError);
                    }
                    if raw >= SYSCALL_ERR_IO {
                        return Err(SysError::Unknown(raw));
                    }
                }
            }

            // Normal character - add to buffer and echo
            _ => {
                if len < buf.len() {
                    buf[len] = ch;
                    len += 1;
                    // Echo the character (already a stack reference)
                    let raw = unsafe {
                        // SAFETY:
                        // - This requires `unsafe` because syscall writes use a raw pointer/length ABI.
                        // - `ch` is a valid stack byte and readable for 1 byte.
                        abi::syscall2(SyscallId::WriteConsole as u64, (&ch as *const u8) as u64, 1)
                    };
                    if raw == SYSCALL_ERR_UNSUPPORTED {
                        return Err(SysError::UnsupportedSyscall);
                    }
                    if raw == SYSCALL_ERR_INVALID_ARG {
                        return Err(SysError::InvalidArgument);
                    }
                    if raw == SYSCALL_ERR_IO {
                        return Err(SysError::IoError);
                    }
                    if raw >= SYSCALL_ERR_IO {
                        return Err(SysError::Unknown(raw));
                    }
                }
            }
        }
    }

    Ok(len)
}
