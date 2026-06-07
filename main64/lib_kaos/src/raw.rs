//! Low-level `int 0x80` syscall stubs.
//!
//! These are `pub(crate)` only — callers in the thematic modules use them to
//! build safe wrappers.  The suffixes (0–3) denote the number of arguments.

use core::arch::asm;

/// Issues `int 0x80` with no arguments beyond the syscall number.
#[inline(always)]
pub(crate) unsafe fn syscall0(syscall_nr: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - Caller ensures this code runs where `int 0x80` is valid (Ring 3).
    // - Register assignment matches the kernel syscall ABI.
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

/// Issues `int 0x80` with one argument.
#[inline(always)]
pub(crate) unsafe fn syscall1(syscall_nr: u64, arg0: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY: See `syscall0`.
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

/// Issues `int 0x80` with two arguments.
#[inline(always)]
pub(crate) unsafe fn syscall2(syscall_nr: u64, arg0: u64, arg1: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY: See `syscall0`.
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

/// Issues `int 0x80` with three arguments.
#[inline(always)]
pub(crate) unsafe fn syscall3(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY: See `syscall0`.
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
