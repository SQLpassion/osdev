//! Minimal user-mode syscall ABI wrappers (`int 0x80`).

use core::arch::asm;

/// Stable syscall numbers exposed by the kernel.
#[repr(u64)]
#[allow(dead_code)]
enum SyscallId {
    Exit = 2,
    WriteConsole = 3,
    GetChar = 4,
}

#[allow(dead_code)]
const SYSCALL_ERR_UNSUPPORTED: u64 = u64::MAX;
#[allow(dead_code)]
const SYSCALL_ERR_INVALID_ARG: u64 = u64::MAX - 1;
#[allow(dead_code)]
const SYSCALL_ERR_IO: u64 = u64::MAX - 2;

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

/// Reads one decoded keyboard character via kernel syscall.
#[inline(always)]
#[allow(dead_code)]
fn getchar() -> Result<u8, u64> {
    let raw = unsafe {
        // SAFETY:
        // - GetChar uses no pointer arguments.
        syscall0(SyscallId::GetChar as u64)
    };

    if raw == SYSCALL_ERR_UNSUPPORTED || raw == SYSCALL_ERR_INVALID_ARG || raw == SYSCALL_ERR_IO {
        return Err(raw);
    }

    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }

    Ok(raw as u8)
}

/// Reads one line from keyboard and echoes input via WriteConsole.
///
/// Newline is echoed but not written into `buf`.
#[inline(always)]
#[allow(dead_code)]
pub fn user_readline(buf: &mut [u8]) -> Result<usize, u64> {
    let mut len = 0usize;

    loop {
        let ch = match getchar() {
            Ok(ch) => ch,
            Err(err) => return Err(err),
        };

        match ch {
            b'\r' | b'\n' => {
                let newline = b'\n';
                let raw = unsafe {
                    // SAFETY:
                    // - `newline` is a valid local byte.
                    write_console((&newline as *const u8).cast(), 1)
                };
                if raw >= SYSCALL_ERR_IO {
                    return Err(raw);
                }
                break;
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    let backspace = 0x08u8;
                    let raw = unsafe {
                        // SAFETY:
                        // - `backspace` is a valid local byte.
                        write_console((&backspace as *const u8).cast(), 1)
                    };
                    if raw >= SYSCALL_ERR_IO {
                        return Err(raw);
                    }
                }
            }
            _ => {
                if len < buf.len() {
                    buf[len] = ch;
                    len += 1;

                    let raw = unsafe {
                        // SAFETY:
                        // - `ch` is a valid local byte.
                        write_console((&ch as *const u8).cast(), 1)
                    };
                    if raw >= SYSCALL_ERR_IO {
                        return Err(raw);
                    }
                }
            }
        }
    }

    Ok(len)
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
