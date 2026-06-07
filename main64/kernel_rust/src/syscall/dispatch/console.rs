//! Console and serial I/O system call implementations.

use core::slice;
use crate::drivers::screen::with_screen;
use crate::drivers::serial::Serial;
use crate::syscall::types::{is_valid_user_buffer, SyscallResult, SyscallError, SYSCALL_OK};

/// Maximum number of bytes that can be written in a single WriteSerial syscall.
/// This limit prevents denial-of-service by bounding syscall execution time
/// and ensures fair CPU scheduling among tasks.
pub const MAX_SERIAL_WRITE_LEN: usize = 4096;
/// Maximum number of bytes that can be written in a single WriteConsole syscall.
/// Same DoS/fairness rationale as `MAX_SERIAL_WRITE_LEN`.
pub const MAX_CONSOLE_WRITE_LEN: usize = 4096;

/// Implements `WriteSerial(ptr, len)`.
///
/// Writes up to `MAX_SERIAL_WRITE_LEN` bytes from the user buffer to COM1.
/// If the requested length exceeds the maximum, only the first
/// `MAX_SERIAL_WRITE_LEN` bytes are written.
pub fn syscall_write_serial_impl(ptr: *const u8, len: usize) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }

    // Step 1: Validate the full claimed range before clamping.
    // A caller whose (ptr, len) overflows, crosses the canonical boundary,
    // is null, or reaches into kernel space receives EINVAL — even though
    // we will subsequently write fewer than `len` bytes due to the DoS cap.
    if !is_valid_user_buffer(ptr, len) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Clamp to maximum to prevent denial-of-service.
    // The full range was validated above; the slice covers a valid sub-range.
    let actual_len = len.min(MAX_SERIAL_WRITE_LEN);

    let bytes = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it builds a slice from a raw userspace pointer.
        // - `is_valid_user_buffer` above verified that `ptr..ptr+len` lies
        //   entirely within user canonical space.
        // - `actual_len <= len`, so `ptr..ptr+actual_len` is a valid sub-range.
        // - `actual_len` is bounded by `MAX_SERIAL_WRITE_LEN`.
        slice::from_raw_parts(ptr, actual_len)
    };

    let serial = Serial::new();

    for byte in bytes {
        serial.write_byte(*byte);
    }

    Ok(actual_len as u64)
}

/// Implements `WriteConsole(ptr, len)`.
///
/// Writes up to `MAX_CONSOLE_WRITE_LEN` bytes from the user buffer to the VGA
/// text console. Semantics mirror `WriteSerial`:
/// - `len == 0` returns `0`,
/// - claimed `(ptr, len)` that overflows, crosses the canonical boundary,
///   or reaches into kernel space returns `SYSCALL_ERR_INVALID_ARG`,
/// - successful call returns number of bytes written (≤ `MAX_CONSOLE_WRITE_LEN`).
pub fn syscall_write_console_impl(ptr: *const u8, len: usize) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }

    // Step 1: Validate the full claimed range before clamping.
    if !is_valid_user_buffer(ptr, len) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Clamp to maximum to prevent denial-of-service.
    let actual_len = len.min(MAX_CONSOLE_WRITE_LEN);

    let bytes = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it builds a slice from a raw userspace pointer.
        // - `is_valid_user_buffer` above verified that `ptr..ptr+len` lies
        //   entirely within user canonical space.
        // - `actual_len <= len`, so `ptr..ptr+actual_len` is a valid sub-range.
        // - `actual_len` is bounded by `MAX_CONSOLE_WRITE_LEN`.
        slice::from_raw_parts(ptr, actual_len)
    };

    with_screen(|screen| {
        for byte in bytes {
            screen.print_char(*byte);
        }
    });

    Ok(actual_len as u64)
}

/// Implements `GetCursor()`.
///
/// Returns the current VGA cursor as a packed 64-bit value:
/// - upper 32 bits: `row`
/// - lower 32 bits: `col`
pub fn syscall_get_cursor_impl() -> SyscallResult<u64> {
    Ok(with_screen(|screen| {
        let (row, col) = screen.get_cursor();
        ((row as u64) << 32) | (col as u64)
    }))
}

/// Implements `SetCursor(row, col)`.
///
/// Sets the VGA cursor position. Values outside the current screen bounds are
/// clamped by the screen driver.
pub fn syscall_set_cursor_impl(row: usize, col: usize) -> SyscallResult<u64> {
    with_screen(|screen| {
        screen.set_cursor(row, col);
    });
    Ok(SYSCALL_OK)
}

/// Implements `ClearScreen()`.
///
/// Clears the entire VGA text buffer and resets the cursor position to `(0, 0)`.
pub fn syscall_clear_screen_impl() -> SyscallResult<u64> {
    with_screen(|screen| {
        screen.clear();
    });
    Ok(SYSCALL_OK)
}
