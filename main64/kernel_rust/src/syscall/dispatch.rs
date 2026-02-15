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

use crate::drivers::screen::with_screen;
use crate::drivers::serial::Serial;
use crate::scheduler;

use super::{
    is_valid_user_buffer, SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_UNSUPPORTED, SYSCALL_OK,
};

/// Maximum number of bytes that can be written in a single WriteSerial syscall.
/// This limit prevents denial-of-service by bounding syscall execution time
/// and ensures fair CPU scheduling among tasks.
const MAX_SERIAL_WRITE_LEN: usize = 4096;
/// Maximum number of bytes that can be written in a single WriteConsole syscall.
/// Same DoS/fairness rationale as `MAX_SERIAL_WRITE_LEN`.
const MAX_CONSOLE_WRITE_LEN: usize = 4096;

/// Resolves syscall number and dispatches to the corresponding kernel handler.
///
/// ABI contract (as set by `int 0x80` entry glue):
/// - `syscall_nr`: `RAX`
/// - `arg0..arg3`: `RDI`, `RSI`, `RDX`, `R10`
///
/// Return contract:
/// - successful calls return syscall-specific values (`SYSCALL_OK` or positive result),
/// - unknown syscall numbers return `SYSCALL_ERR_UNSUPPORTED`.
pub fn dispatch(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    match syscall_nr {
        SyscallId::YIELD => syscall_yield_impl(),
        SyscallId::WRITE_SERIAL => {
            syscall_write_serial_impl(arg0 as *const u8, arg1 as usize)
        }
        SyscallId::WRITE_CONSOLE => {
            syscall_write_console_impl(arg0 as *const u8, arg1 as usize)
        }
        SyscallId::EXIT => syscall_exit_impl(),
        _ => {
            // Silence unused parameter warnings for future syscalls
            let _ = (arg2, arg3);
            SYSCALL_ERR_UNSUPPORTED
        }
    }
}

/// Implements `Yield`: cooperative handoff to scheduler.
///
/// This function only returns the result code — it does **not** trigger the
/// reschedule itself.  The actual context switch is performed by the caller
/// [`syscall_rust_dispatch`](crate::arch::interrupts::syscall_rust_dispatch),
/// which calls [`on_timer_tick`](crate::scheduler::on_timer_tick) directly
/// with the current interrupt frame after `dispatch` returns.
///
/// # Why not call `yield_now()` here?
///
/// `yield_now()` issues `int 32` (PIT timer vector) to enter the scheduler.
/// When called from inside the `int 0x80` handler, this would create a
/// **nested interrupt**: the CPU pushes a second IRET frame and a second
/// register save onto the same kernel stack.  This has three problems:
///
/// 1. **Double stack consumption** — two full register saves plus two IRET
///    frames (~320 bytes) per yield, eating into the 64 KiB task kernel stack.
/// 2. **Unnecessary overhead** — two interrupt entry/exit round-trips instead
///    of one.
/// 3. **Fragility** — the scheduler sees the inner `int 32` frame rather than
///    the original `int 0x80` frame that holds the actual user-mode context.
///
/// By returning `SYSCALL_OK` here and letting `syscall_rust_dispatch` feed
/// the *original* `int 0x80` frame into `on_timer_tick`, the scheduler sees
/// the correct user context and can switch tasks with a single `iretq`.
fn syscall_yield_impl() -> u64 {
    SYSCALL_OK
}

/// Implements `WriteSerial(ptr, len)`.
///
/// Writes up to `MAX_SERIAL_WRITE_LEN` bytes from the user buffer to COM1.
/// If the requested length exceeds the maximum, only the first
/// `MAX_SERIAL_WRITE_LEN` bytes are written.
///
/// Behavior:
/// - `len == 0` is treated as success and returns `0`,
/// - null pointer with non-zero `len` returns `SYSCALL_ERR_INVALID_ARG`,
/// - invalid user buffer returns `SYSCALL_ERR_INVALID_ARG`,
/// - otherwise bytes are read from caller memory and written to COM1,
///   returning the number of bytes actually written.
///
/// # DoS Protection
/// The maximum write length prevents a single syscall from monopolizing
/// the CPU for an unbounded duration. User code must chunk large writes
/// into multiple syscalls.
fn syscall_write_serial_impl(ptr: *const u8, len: usize) -> u64 {
    if len == 0 {
        return 0;
    }

    // Clamp to maximum to prevent denial-of-service.
    // User code must chunk large buffers across multiple syscalls.
    let actual_len = len.min(MAX_SERIAL_WRITE_LEN);

    // Reject kernel-half addresses, null pointers, and overflow attempts.
    // Actual page mappability is enforced by the MMU at access time.
    if !is_valid_user_buffer(ptr, actual_len) {
        return SYSCALL_ERR_INVALID_ARG;
    }

    let bytes = unsafe {
        // SAFETY:
        // - `is_valid_user_buffer` above verified that `ptr..ptr+actual_len` lies
        //   entirely within user canonical space.
        // - `actual_len` is bounded by `MAX_SERIAL_WRITE_LEN`.
        slice::from_raw_parts(ptr, actual_len)
    };

    let serial = Serial::new();

    for byte in bytes {
        serial.write_byte(*byte);
    }

    actual_len as u64
}

/// Implements `WriteConsole(ptr, len)`.
///
/// Writes up to `MAX_CONSOLE_WRITE_LEN` bytes from the user buffer to the VGA
/// text console. Semantics mirror `WriteSerial`:
/// - `len == 0` returns `0`,
/// - invalid pointer/range returns `SYSCALL_ERR_INVALID_ARG`,
/// - successful call returns number of bytes written.
///
/// Bytes are written as raw VGA text characters; this syscall does not enforce
/// UTF-8 validity and is intended for simple ASCII/debug output.
fn syscall_write_console_impl(ptr: *const u8, len: usize) -> u64 {
    if len == 0 {
        return 0;
    }

    let actual_len = len.min(MAX_CONSOLE_WRITE_LEN);
    if !is_valid_user_buffer(ptr, actual_len) {
        return SYSCALL_ERR_INVALID_ARG;
    }

    let bytes = unsafe {
        // SAFETY:
        // - `is_valid_user_buffer` above verified that `ptr..ptr+actual_len` lies
        //   entirely within user canonical space.
        // - `actual_len` is bounded by `MAX_CONSOLE_WRITE_LEN`.
        slice::from_raw_parts(ptr, actual_len)
    };

    with_screen(|screen| {
        for byte in bytes {
            screen.print_char(*byte);
        }
    });

    actual_len as u64
}

/// Implements `Exit()`.
///
/// Marks the current task as [`Zombie`](crate::scheduler::TaskState::Zombie)
/// and returns `SYSCALL_OK`. The actual reschedule is driven by
/// [`syscall_rust_dispatch`](crate::arch::interrupts::syscall_rust_dispatch),
/// which calls [`on_timer_tick`](crate::scheduler::on_timer_tick) directly —
/// analogous to the Yield path.
///
/// The zombie task will never be selected again and is reaped on the
/// following scheduler tick once execution has moved off its kernel stack.
///
/// # Exit Code
/// This syscall does not accept an exit code parameter. If future support
/// for process wait semantics is added, the exit code parameter can be
/// reintroduced and stored in the task entry for retrieval by a parent task.
fn syscall_exit_impl() -> u64 {
    scheduler::mark_current_as_zombie();
    SYSCALL_OK
}
