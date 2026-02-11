//! Kernel-side syscall dispatcher (`int 0x80` path).
//!
//! Responsibilities of this module:
//! - decode syscall number + ABI arguments,
//! - route to the corresponding kernel implementation,
//! - enforce minimal argument validation at syscall boundaries,
//! - return stable numeric result/error codes to caller context.
//!
//! ABI for `dispatch` (provided by interrupt entry glue):
//! - `RAX` -> `syscall_nr`
//! - `RDI` -> `arg0`
//! - `RSI` -> `arg1`
//! - `RDX` -> `arg2`
//! - `R10` -> `arg3`

use core::slice;

use crate::drivers::serial::Serial;
use crate::scheduler;

use super::{SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_UNSUPPORTED, SYSCALL_OK};

/// Resolves syscall number and dispatches to the corresponding kernel handler.
///
/// ABI contract (as set by `int 0x80` entry glue):
/// - `syscall_nr`: `RAX`
/// - `arg0..arg3`: `RDI`, `RSI`, `RDX`, `R10`
///
/// Return contract:
/// - successful calls return syscall-specific values (`SYSCALL_OK` or positive result),
/// - unknown syscall numbers return `SYSCALL_ERR_UNSUPPORTED`.
pub fn dispatch(syscall_nr: u64, arg0: u64, arg1: u64, _arg2: u64, _arg3: u64) -> u64 {
    match syscall_nr {
        // NOTE:
        // `SyscallId::X as u64` is an expression, not a pattern.
        // Rust `match` arms require patterns, so enum->u64 comparisons must be
        // expressed via guards unless we introduce separate `const` values.
        x if x == SyscallId::Yield as u64 => syscall_yield_impl(),
        x if x == SyscallId::WriteSerial as u64 => {
            syscall_write_serial_impl(arg0 as *const u8, arg1 as usize)
        }
        x if x == SyscallId::Exit as u64 => syscall_exit_impl(arg0),
        _ => SYSCALL_ERR_UNSUPPORTED,
    }
}

/// Implements `Yield`: cooperative handoff to scheduler.
///
/// Returns `SYSCALL_OK` once control resumes in this task context.
fn syscall_yield_impl() -> u64 {
    scheduler::yield_now();
    
    SYSCALL_OK
}

/// Implements `WriteSerial(ptr, len)`.
///
/// Behavior:
/// - `len == 0` is treated as success and returns `0`,
/// - null pointer with non-zero `len` returns `SYSCALL_ERR_INVALID_ARG`,
/// - otherwise bytes are read from caller memory and written to COM1,
///   returning number of written bytes.
fn syscall_write_serial_impl(ptr: *const u8, len: usize) -> u64 {
    if len == 0 {
        return 0;
    }

    if ptr.is_null() {
        return SYSCALL_ERR_INVALID_ARG;
    }

    let bytes = unsafe {
        // SAFETY:
        // - Caller must pass a readable user buffer for `len` bytes.
        // - Null pointer is rejected above.
        slice::from_raw_parts(ptr, len)
    };

    let serial = Serial::new();

    for byte in bytes {
        serial.write_byte(*byte);
    }

    len as u64
}

/// Implements `Exit(exit_code)`.
///
/// The scheduler path is expected to terminate the current task and never
/// resume it; this function therefore does not provide a meaningful success
/// return to the exiting caller context.
fn syscall_exit_impl(_exit_code: u64) -> u64 {
    scheduler::exit_current_task()
}
