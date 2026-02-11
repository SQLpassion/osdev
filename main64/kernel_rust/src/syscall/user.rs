//! User-space oriented syscall wrappers.
//!
//! This module provides ergonomic wrappers around the raw `int 0x80` ABI:
//! - `sys_yield` for cooperative scheduling,
//! - `sys_write_serial` for debug output,
//! - `sys_exit` to terminate the current task.
//!
//! Design goals:
//! - keep call sites simple (`Result`-based API where possible),
//! - centralize syscall return decoding,
//! - keep unsafe and ABI details local to wrapper implementations.

use core::arch::asm;

use super::{abi, decode_result, SysError, SyscallId};

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
    
    decode_result(raw_value).map(|_| ())
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
    let raw_value = unsafe {
        // SAFETY:
        // - Pointer/length are derived from a valid Rust slice.
        abi::syscall2(SyscallId::WriteSerial as u64, buf.as_ptr() as u64, buf.len() as u64)
    };

    decode_result(raw_value).map(|written| written as usize)
}

/// Terminates the current task with `exit_code`.
///
/// Expected behavior:
/// - on a correct scheduler path, this never returns,
/// - if it unexpectedly returns, the function enters a local terminal loop.
#[inline(always)]
pub fn sys_exit(exit_code: u64) -> ! {
    let _ = unsafe {
        // SAFETY:
        // - Wrapper is intended for contexts where `int 0x80` exit syscall is available.
        abi::syscall1(SyscallId::Exit as u64, exit_code)
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
