//! VGA console, serial port, and keyboard input syscall wrappers.

use crate::{
    decode_result,
    raw::{syscall0, syscall1, syscall2},
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

/// Extended key event decoded from the `ReadKey` syscall return value.
///
/// The kernel encodes key events as a single byte:
/// - `0x01`–`0x7F` → `Char(byte)` (printable ASCII)
/// - `0x80`        → `Escape`
/// - `0x81`        → `Backspace`
/// - `0x82`        → `Enter`
/// - `0x83`        → `ArrowUp`
/// - `0x84`        → `ArrowDown`
/// - `0x85`        → `ArrowLeft`
/// - `0x86`        → `ArrowRight`
/// - `0x90`–`0x9B` → `F(1)`–`F(12)`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Unknown,
    Char(u8),
    Escape,
    Backspace,
    Enter,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F(u8),
}

impl Key {
    fn from_raw(byte: u8) -> Self {
        match byte {
            0x00        => Key::Unknown,
            0x80        => Key::Escape,
            0x81        => Key::Backspace,
            0x82        => Key::Enter,
            0x83        => Key::ArrowUp,
            0x84        => Key::ArrowDown,
            0x85        => Key::ArrowLeft,
            0x86        => Key::ArrowRight,
            b if b >= 0x90 => Key::F(b.wrapping_sub(0x8F)),
            b           => Key::Char(b),
        }
    }
}

/// Read a single extended key event from the keyboard (blocking).
///
/// Blocks until a key is available, then decodes and returns it.
#[inline(always)]
pub fn read_key() -> Result<Key, SysError> {
    let raw = unsafe {
        // SAFETY: `ReadKey` takes no pointer arguments.
        syscall0(SyscallId::ReadKey as u64)
    };
    decode_result(raw).map(|v| Key::from_raw(v as u8))
}

/// Non-blocking check for a keyboard key event.
///
/// Returns `Ok(Key::Char(byte))` etc if a key is available, or `Ok(Key::Unknown)` if empty.
#[inline(always)]
pub fn poll_key() -> Result<Key, SysError> {
    let raw = unsafe {
        // SAFETY: `PollKey` takes no pointer arguments.
        syscall0(SyscallId::PollKey as u64)
    };

    decode_result(raw).map(|v| Key::from_raw(v as u8))
}

/// Blit a raw frame buffer to the console in a single syscall.
///
/// Each element of `cells` encodes one cell as `(attr << 8) | ascii`.
#[inline(always)]
pub fn flush_screen(cells: &[u16]) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `cells` is a valid slice; pointer and length are
        //         passed to the kernel for a bounds-checked read.
        syscall2(
            SyscallId::WriteFramebuffer as u64,
            cells.as_ptr() as u64,
            cells.len() as u64,
        )
    };
    decode_result(raw).map(|_| ())
}

/// Retrieve the active console's dimensions as a tuple `(rows, cols)`.
#[inline(always)]
pub fn get_dimensions() -> Result<(usize, usize), SysError> {
    let raw = unsafe {
        // SAFETY: `GetConsoleDimensions` takes no pointer arguments.
        syscall0(SyscallId::GetConsoleDimensions as u64)
    };
    decode_result(raw).map(|val| {
        let rows = (val >> 32) as usize;
        let cols = (val & 0xFFFFFFFF) as usize;
        (rows, cols)
    })
}

/// Configure VGA text-mode hardware settings.
///
/// `flags` bitmap:
/// - bit 0: hardware cursor (1 = enabled, 0 = disabled)
/// - bit 1: blink mode     (1 = enabled, 0 = disabled)
///
/// Use `0b00` to enter TUI mode (cursor + blink off), `0b11` to restore defaults.
#[inline(always)]
pub fn set_vga_mode(flags: u64) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `SetVgaMode` takes only an integer flag argument (no pointers).
        syscall1(SyscallId::SetVgaMode as u64, flags)
    };
    decode_result(raw).map(|_| ())
}
