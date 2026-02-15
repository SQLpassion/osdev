//! User-space oriented syscall wrappers.
//!
//! This module provides ergonomic wrappers around the raw `int 0x80` ABI:
//! - `sys_yield` for cooperative scheduling,
//! - `sys_write_serial` for debug output,
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

/// Requests a cooperative reschedule.
///
/// This invokes syscall `Yield` and returns `Ok(())` on success.
/// Any kernel error sentinel is translated to `SysError`.
#[inline(always)]
pub fn sys_yield() -> Result<(), SysError> {
    let raw_value = unsafe {
        // SAFETY:
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

/// Writes `buf` to the kernel debug serial output (COM1).
///
/// ABI arguments:
/// - `arg0` (`RDI`) = `buf.as_ptr()`
/// - `arg1` (`RSI`) = `buf.len()`
///
/// Return value:
/// - `Ok(written)` with number of written bytes,
/// - `Err(...)` when kernel reports a syscall error.
#[inline(always)]
pub fn sys_write_serial(buf: &[u8]) -> Result<usize, SysError> {
    unsafe { sys_write_serial_raw(buf.as_ptr(), buf.len()) }
}

/// Writes `len` bytes from `ptr` to the kernel debug serial output (COM1).
///
/// # Safety
/// Caller must ensure that `ptr..ptr+len` is readable in the current
/// user/kernel context expected by the syscall boundary.
#[inline(always)]
pub unsafe fn sys_write_serial_raw(ptr: *const u8, len: usize) -> Result<usize, SysError> {
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
        // - Wrapper is intended for contexts where `int 0x80` exit syscall is available.
        abi::syscall0(SyscallId::Exit as u64)
    };
    unsafe {
        // SAFETY:
        // - Executed only as fallback if `sys_exit` unexpectedly returns.
        // - Keeps control in a local tight loop without calling external code.
        asm!(
            "2:",
            "pause",
            "jmp 2b",
            options(noreturn)
        );
    }
}
