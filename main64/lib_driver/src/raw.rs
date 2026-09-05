//! Low-level `int 0x80` syscall stubs for driver routines.

#[cfg(target_arch = "x86_64")]
use core::arch::asm;

/// Issues `int 0x80` with one argument.
#[inline(always)]
pub(crate) unsafe fn syscall1(syscall_nr: u64, arg0: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut ret = syscall_nr;

        // SAFETY:
        // - Caller ensures this code runs in Ring 3 where `int 0x80` is handled by the kernel.
        // - Registers match the kernel's syscall dispatch convention.
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
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (syscall_nr, arg0);
        0
    }
}

/// Issues `int 0x80` with two arguments.
#[inline(always)]
pub(crate) unsafe fn syscall2(syscall_nr: u64, arg0: u64, arg1: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut ret = syscall_nr;

        // SAFETY:
        // - Caller ensures Ring-3 execution.
        // - Register parameters adhere to the syscall ABI.
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
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (syscall_nr, arg0, arg1);
        0
    }
}

/// Issues `int 0x80` with three arguments.
#[inline(always)]
pub(crate) unsafe fn syscall3(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut ret = syscall_nr;

        // SAFETY:
        // - Caller ensures Ring-3 execution.
        // - Register parameters adhere to the syscall ABI.
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
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (syscall_nr, arg0, arg1, arg2);
        0
    }
}

/// Issues `int 0x80` with four arguments.
#[inline(always)]
pub(crate) unsafe fn syscall4(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let mut ret = syscall_nr;

        // SAFETY:
        // - Caller ensures Ring-3 execution.
        // - Register parameters adhere to the syscall ABI.
        unsafe {
            asm!(
                "int 0x80",
                inout("rax") ret,
                in("rdi") arg0,
                in("rsi") arg1,
                in("rdx") arg2,
                in("r10") arg3
            );
        }
        ret
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (syscall_nr, arg0, arg1, arg2, arg3);
        0
    }
}
