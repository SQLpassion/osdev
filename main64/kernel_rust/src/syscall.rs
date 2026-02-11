//! Syscall table and dispatcher for the `int 0x80` entry path.
//!
//! The low-level interrupt glue passes `(syscall_nr, arg0..arg3)` into
//! [`dispatch`]. This module resolves the syscall number and routes to the
//! corresponding kernel implementation.

use core::slice;

use crate::drivers::serial::Serial;
use crate::scheduler;

/// Stable syscall numbers exposed to user mode.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallId {
    /// Cooperative reschedule request.
    Yield = 0,
    /// Write bytes to debug serial (COM1).
    WriteSerial = 1,
    /// Terminate current task.
    Exit = 2,
}

/// Unknown syscall number.
pub const ERR_ENOSYS: u64 = u64::MAX;
/// Invalid argument combination for a known syscall.
pub const ERR_EINVAL: u64 = u64::MAX - 1;
/// Successful syscall return code for void-like operations.
pub const OK: u64 = 0;

/// Resolve syscall number and dispatch to the corresponding handler.
///
/// ABI contract (as set by `int 0x80` entry glue):
/// - `syscall_nr`: `RAX`
/// - `arg0..arg3`: `RDI`, `RSI`, `RDX`, `R10`
pub fn dispatch(syscall_nr: u64, arg0: u64, arg1: u64, _arg2: u64, _arg3: u64) -> u64 {
    match syscall_nr {
        x if x == SyscallId::Yield as u64 => syscall_yield_impl(),
        x if x == SyscallId::WriteSerial as u64 => {
            syscall_write_serial_impl(arg0 as *const u8, arg1 as usize)
        }
        x if x == SyscallId::Exit as u64 => syscall_exit_impl(arg0),
        _ => ERR_ENOSYS,
    }
}

fn syscall_yield_impl() -> u64 {
    scheduler::yield_now();
    OK
}

fn syscall_write_serial_impl(ptr: *const u8, len: usize) -> u64 {
    if len == 0 {
        return 0;
    }
    if ptr.is_null() {
        return ERR_EINVAL;
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

fn syscall_exit_impl(_exit_code: u64) -> u64 {
    scheduler::exit_current_task()
}
