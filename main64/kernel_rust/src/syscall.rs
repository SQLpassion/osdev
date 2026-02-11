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
/// Maximum encoded length of the `write_serial -> exit` user stub.
#[allow(dead_code)]
pub const USER_SERIAL_EXIT_STUB_MAX_LEN: usize = 62;

/// Computes the user-mode alias RIP for a kernel function page mapped at `code_page_user_va`.
///
/// The returned address keeps the original 4 KiB page offset of `kernel_entry_va`.
#[inline]
#[allow(dead_code)]
pub const fn user_alias_rip(code_page_user_va: u64, kernel_entry_va: u64) -> u64 {
    code_page_user_va + (kernel_entry_va & 0xFFF)
}

/// Maps a kernel virtual address into a user-code alias window.
///
/// Returns `None` when `kernel_va` is below `kernel_base` or when the offset
/// does not fit into the provided user code window size.
#[inline]
pub const fn user_alias_va_for_kernel(
    user_code_base: u64,
    user_code_size: u64,
    kernel_base: u64,
    kernel_va: u64,
) -> Option<u64> {
    if kernel_va < kernel_base {
        return None;
    }
    let offset = kernel_va - kernel_base;
    if offset >= user_code_size {
        return None;
    }
    Some(user_code_base + offset)
}

/// User-facing syscall error space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysError {
    /// Unknown syscall number.
    Enosys,
    /// Invalid syscall arguments.
    Einval,
    /// Any unclassified kernel return value in the error range.
    Unknown(u64),
}

/// Decodes a raw syscall return value into `Result`.
#[inline]
#[allow(dead_code)]
pub fn decode_result(raw: u64) -> Result<u64, SysError> {
    match raw {
        ERR_ENOSYS => Err(SysError::Enosys),
        ERR_EINVAL => Err(SysError::Einval),
        x if x >= ERR_EINVAL => Err(SysError::Unknown(x)),
        value => Ok(value),
    }
}

/// Raw architecture syscall entry helpers (`int 0x80` ABI).
pub mod arch {
    pub mod syscall_raw {
        use core::arch::asm;

        /// Executes a zero-argument syscall.
        #[inline(always)]
        pub unsafe fn raw0(syscall_nr: u64) -> u64 {
            let mut ret = syscall_nr;
            // SAFETY:
            // - Caller guarantees the current CPU mode may legally execute `int 0x80`.
            // - Register assignment follows the kernel ABI contract.
            unsafe {
                asm!(
                    "int 0x80",
                    inout("rax") ret,
                    in("rdi") 0u64,
                    in("rsi") 0u64,
                    in("rdx") 0u64,
                    in("r10") 0u64,
                    options(nostack)
                );
            }
            ret
        }

        /// Executes a one-argument syscall.
        #[inline(always)]
        pub unsafe fn raw1(syscall_nr: u64, arg0: u64) -> u64 {
            let mut ret = syscall_nr;
            // SAFETY:
            // - Caller guarantees the current CPU mode may legally execute `int 0x80`.
            // - Register assignment follows the kernel ABI contract.
            unsafe {
                asm!(
                    "int 0x80",
                    inout("rax") ret,
                    in("rdi") arg0,
                    in("rsi") 0u64,
                    in("rdx") 0u64,
                    in("r10") 0u64,
                    options(nostack)
                );
            }
            ret
        }

        /// Executes a two-argument syscall.
        #[inline(always)]
        pub unsafe fn raw2(syscall_nr: u64, arg0: u64, arg1: u64) -> u64 {
            let mut ret = syscall_nr;
            // SAFETY:
            // - Caller guarantees the current CPU mode may legally execute `int 0x80`.
            // - Register assignment follows the kernel ABI contract.
            unsafe {
                asm!(
                    "int 0x80",
                    inout("rax") ret,
                    in("rdi") arg0,
                    in("rsi") arg1,
                    in("rdx") 0u64,
                    in("r10") 0u64,
                    options(nostack)
                );
            }
            ret
        }
    }
}

/// Safe user-space syscall wrappers.
#[allow(dead_code)]
pub mod user {
    use core::arch::asm;

    use super::{arch::syscall_raw, SysError, SyscallId, ERR_EINVAL, ERR_ENOSYS};

    #[inline(always)]
    fn decode_inline(raw: u64) -> Result<u64, SysError> {
        if raw == ERR_ENOSYS {
            return Err(SysError::Enosys);
        }
        if raw == ERR_EINVAL {
            return Err(SysError::Einval);
        }
        if raw >= ERR_EINVAL {
            return Err(SysError::Unknown(raw));
        }
        Ok(raw)
    }

    /// Cooperative yield to the scheduler.
    #[inline(always)]
    pub fn sys_yield() -> Result<(), SysError> {
        let raw = unsafe {
            // SAFETY:
            // - Wrapper is intended for ring-3/ring-0 contexts where `int 0x80` is configured.
            syscall_raw::raw0(SyscallId::Yield as u64)
        };
        decode_inline(raw).map(|_| ())
    }

    /// Writes bytes to the kernel debug serial output from raw pointer + length.
    ///
    /// # Safety
    /// Caller must ensure `ptr..ptr+len` is readable in the current address space.
    #[inline(always)]
    pub unsafe fn sys_write_serial_raw(ptr: *const u8, len: usize) -> Result<usize, SysError> {
        let raw = unsafe {
            // SAFETY:
            // - Caller guarantees buffer validity for `len` bytes.
            syscall_raw::raw2(SyscallId::WriteSerial as u64, ptr as u64, len as u64)
        };
        decode_inline(raw).map(|written| written as usize)
    }

    /// Writes bytes to the kernel debug serial output.
    #[inline(always)]
    pub fn sys_write_serial(buf: &[u8]) -> Result<usize, SysError> {
        unsafe {
            // SAFETY:
            // - Pointer/length are derived from a valid Rust slice.
            sys_write_serial_raw(buf.as_ptr(), buf.len())
        }
    }

    /// Terminates the current task.
    ///
    /// On a correct kernel scheduler path this call does not return.
    #[inline(always)]
    pub fn sys_exit(exit_code: u64) -> ! {
        let _ = unsafe {
            // SAFETY:
            // - Wrapper is intended for contexts where `int 0x80` exit syscall is available.
            syscall_raw::raw1(SyscallId::Exit as u64, exit_code)
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
}

#[inline]
#[allow(dead_code)]
fn emit_bytes(out: &mut [u8], cursor: &mut usize, bytes: &[u8]) -> bool {
    let end = *cursor + bytes.len();
    if end > out.len() {
        return false;
    }
    out[*cursor..end].copy_from_slice(bytes);
    *cursor = end;
    true
}

#[inline]
#[allow(dead_code)]
fn emit_u64(out: &mut [u8], cursor: &mut usize, value: u64) -> bool {
    emit_bytes(out, cursor, &value.to_le_bytes())
}

/// Encodes a tiny ring-3 stub that:
/// 1. calls `WriteSerial(message_va, message_len)`,
/// 2. calls `Exit(exit_code)`,
/// 3. falls into `jmp $` as terminal safety loop.
///
/// Returns encoded byte length on success, otherwise `None` if `out` is too small.
#[allow(dead_code)]
pub fn encode_user_serial_then_exit_stub(
    out: &mut [u8],
    message_va: u64,
    message_len: usize,
    exit_code: u64,
) -> Option<usize> {
    let mut cursor = 0usize;

    // mov rax, SyscallId::WriteSerial
    if !emit_bytes(out, &mut cursor, &[0x48, 0xB8]) {
        return None;
    }
    if !emit_u64(out, &mut cursor, SyscallId::WriteSerial as u64) {
        return None;
    }
    // mov rdi, message_va
    if !emit_bytes(out, &mut cursor, &[0x48, 0xBF]) {
        return None;
    }
    if !emit_u64(out, &mut cursor, message_va) {
        return None;
    }
    // mov rsi, message_len
    if !emit_bytes(out, &mut cursor, &[0x48, 0xBE]) {
        return None;
    }
    if !emit_u64(out, &mut cursor, message_len as u64) {
        return None;
    }
    // xor r10, r10
    if !emit_bytes(out, &mut cursor, &[0x4D, 0x31, 0xD2]) {
        return None;
    }
    // int 0x80
    if !emit_bytes(out, &mut cursor, &[0xCD, 0x80]) {
        return None;
    }

    // mov rax, SyscallId::Exit
    if !emit_bytes(out, &mut cursor, &[0x48, 0xB8]) {
        return None;
    }
    if !emit_u64(out, &mut cursor, SyscallId::Exit as u64) {
        return None;
    }
    // mov rdi, exit_code
    if !emit_bytes(out, &mut cursor, &[0x48, 0xBF]) {
        return None;
    }
    if !emit_u64(out, &mut cursor, exit_code) {
        return None;
    }
    // xor r10, r10
    if !emit_bytes(out, &mut cursor, &[0x4D, 0x31, 0xD2]) {
        return None;
    }
    // int 0x80
    if !emit_bytes(out, &mut cursor, &[0xCD, 0x80]) {
        return None;
    }
    // jmp $
    if !emit_bytes(out, &mut cursor, &[0xEB, 0xFE]) {
        return None;
    }

    Some(cursor)
}

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
