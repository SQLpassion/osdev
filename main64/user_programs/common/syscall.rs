//! Minimal user-mode syscall ABI wrappers (`int 0x80`).

use core::arch::asm;

/// Stable syscall numbers exposed by the kernel.
#[repr(u64)]
enum SyscallId {
    Exit = 2,
    WriteConsole = 3,
}

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

/// Writes bytes to VGA console via kernel syscall.
#[inline(always)]
pub unsafe fn write_console(ptr: *const u8, len: usize) -> u64 {
    // SAFETY:
    // - Caller provides buffer pointer/length according to syscall contract.
    unsafe { syscall2(SyscallId::WriteConsole as u64, ptr as u64, len as u64) }
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
