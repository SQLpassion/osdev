//! Raw architecture syscall entry helpers (`int 0x80` ABI).

use core::arch::asm;

/// Executes a zero-argument syscall.
///
/// # Safety
/// Caller must ensure that executing `int 0x80` is valid in the current CPU
/// context and that the kernel syscall ABI is initialized.
#[inline(always)]
pub unsafe fn syscall0(syscall_nr: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Caller guarantees the current CPU mode may legally execute `int 0x80`.
    // - Register assignment follows the kernel ABI contract.
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

/// Executes a one-argument syscall.
///
/// # Safety
/// Caller must ensure that executing `int 0x80` is valid in the current CPU
/// context and that `arg0` is a valid value for the targeted syscall ABI.
#[inline(always)]
pub unsafe fn syscall1(syscall_nr: u64, arg0: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Caller guarantees the current CPU mode may legally execute `int 0x80`.
    // - Register assignment follows the kernel ABI contract.
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

/// Executes a two-argument syscall.
///
/// # Safety
/// Caller must ensure that executing `int 0x80` is valid in the current CPU
/// context and that `arg0`/`arg1` satisfy the targeted syscall ABI contract.
#[inline(always)]
pub unsafe fn syscall2(syscall_nr: u64, arg0: u64, arg1: u64) -> u64 {
    let mut ret = syscall_nr;

    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Caller guarantees the current CPU mode may legally execute `int 0x80`.
    // - Register assignment follows the kernel ABI contract.
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
