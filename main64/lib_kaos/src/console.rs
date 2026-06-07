//! VGA console, serial port, and keyboard input syscall wrappers.

use crate::{
    decode_result,
    raw::{syscall0, syscall2},
    SyscallId, SysError,
};

/// Writes `msg` to the VGA text console.
#[inline(always)]
pub fn writeline(msg: &[u8]) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `msg` is a valid slice whose pointer and length are passed to the kernel.
        syscall2(SyscallId::WriteConsole as u64, msg.as_ptr() as u64, msg.len() as u64)
    };
    decode_result(raw).map(|_| ())
}

/// Writes `msg` to the debug serial port (COM1).
#[inline(always)]
pub fn write_serial(msg: &[u8]) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `msg` is a valid slice whose pointer and length are passed to the kernel.
        syscall2(SyscallId::WriteSerial as u64, msg.as_ptr() as u64, msg.len() as u64)
    };
    decode_result(raw).map(|_| ())
}

/// Clears the VGA text screen and resets the cursor to the origin.
#[inline(always)]
pub fn clear_screen() -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `ClearScreen` takes no pointer arguments.
        syscall0(SyscallId::ClearScreen as u64)
    };
    decode_result(raw).map(|_| ())
}

/// Reads one decoded keyboard character (blocking).
#[inline(always)]
#[allow(clippy::cast_possible_truncation)]
fn getchar() -> Result<u8, SysError> {
    let raw = unsafe {
        // SAFETY: `GetChar` takes no pointer arguments.
        syscall0(SyscallId::GetChar as u64)
    };
    decode_result(raw).map(|val| val as u8)
}

/// Reads one line from the keyboard, echoes every character to the console.
///
/// The terminating newline is echoed but **not** stored in `buf`.
/// Returns the number of bytes written into `buf`.
#[inline(always)]
#[allow(clippy::cast_possible_truncation)]
pub fn readline(buf: &mut [u8]) -> Result<usize, SysError> {
    let mut len = 0usize;

    loop {
        let ch = getchar()?;

        match ch {
            b'\r' | b'\n' => {
                let newline = b'\n';
                let raw = unsafe {
                    // SAFETY: `newline` is a valid local byte on the stack.
                    crate::raw::syscall2(
                        SyscallId::WriteConsole as u64,
                        &raw const newline as u64,
                        1,
                    )
                };
                decode_result(raw)?;
                break;
            }
            0x08 => {
                // Backspace
                if len > 0 {
                    len -= 1;
                    let bs = 0x08u8;
                    let raw = unsafe {
                        // SAFETY: `bs` is a valid local byte on the stack.
                        crate::raw::syscall2(
                            SyscallId::WriteConsole as u64,
                            &raw const bs as u64,
                            1,
                        )
                    };
                    decode_result(raw)?;
                }
            }
            _ => {
                if len < buf.len() {
                    buf[len] = ch;
                    len += 1;
                    let raw = unsafe {
                        // SAFETY: `ch` is a valid local byte on the stack.
                        crate::raw::syscall2(
                            SyscallId::WriteConsole as u64,
                            &raw const ch as u64,
                            1,
                        )
                    };
                    decode_result(raw)?;
                }
            }
        }
    }

    Ok(len)
}

#[doc(hidden)]
pub struct ConsoleWriter;

impl core::fmt::Write for ConsoleWriter {
    #[inline(always)]
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let _ = writeline(s.as_bytes());
        Ok(())
    }
}

#[doc(hidden)]
#[inline(always)]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    let _ = ConsoleWriter.write_fmt(args);
}
