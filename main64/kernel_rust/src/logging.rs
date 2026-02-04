//! Central kernel logging with optional in-memory capture for console dump.

use core::cell::UnsafeCell;
use core::fmt::{self, Write as _};

use crate::drivers::screen::{Color, Screen};
use crate::drivers::serial;

const CAPTURE_BUF_SIZE: usize = 16 * 1024;

struct LogState {
    capture_enabled: bool,
    capture_len: usize,
    capture_overflow: bool,
    capture_buf: [u8; CAPTURE_BUF_SIZE],
}

struct GlobalLogger {
    inner: UnsafeCell<LogState>,
}

impl GlobalLogger {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(LogState {
                capture_enabled: false,
                capture_len: 0,
                capture_overflow: false,
                capture_buf: [0; CAPTURE_BUF_SIZE],
            }),
        }
    }
}

// Safety: Kernel is effectively single-threaded (no SMP).
unsafe impl Sync for GlobalLogger {}

static LOGGER: GlobalLogger = GlobalLogger::new();

struct BufferWriter<'a> {
    state: &'a mut LogState,
}

impl fmt::Write for BufferWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self
            .state
            .capture_buf
            .len()
            .saturating_sub(self.state.capture_len);
        let write_len = remaining.min(bytes.len());

        if write_len > 0 {
            let start = self.state.capture_len;
            let end = start + write_len;
            self.state.capture_buf[start..end].copy_from_slice(&bytes[..write_len]);
            self.state.capture_len = end;
        }

        if write_len < bytes.len() {
            self.state.capture_overflow = true;
        }
        Ok(())
    }
}

fn with_logger<R>(f: impl FnOnce(&mut LogState) -> R) -> R {
    unsafe { f(&mut *LOGGER.inner.get()) }
}

fn capture_target_line(target: &str, args: fmt::Arguments<'_>) {
    with_logger(|state| {
        if !state.capture_enabled {
            return;
        }

        let mut writer = BufferWriter { state };
        let _ = writer.write_str(target);
        let _ = writer.write_char('|');
        let _ = fmt::write(&mut writer, args);
        let _ = writer.write_char('\n');
    });
}

/// Central target-based log function (serial output + optional capture).
pub fn logln(target: &str, args: fmt::Arguments<'_>) {
    serial::_debug_print(format_args!("{}\n", args));
    capture_target_line(target, args);
}

/// Enable/disable capture buffer and reset it.
pub fn set_capture_enabled(enabled: bool) {
    with_logger(|state| {
        state.capture_enabled = enabled;
        state.capture_len = 0;
        state.capture_overflow = false;
    });
}

/// Dump captured logs for one target to the console.
pub fn print_captured_target(
    screen: &mut Screen,
    target: &str,
    mut highlight: impl FnMut(&str) -> bool,
) {
    let (ptr, len, overflow) = with_logger(|state| {
        (
            state.capture_buf.as_ptr(),
            state.capture_len,
            state.capture_overflow,
        )
    });

    if len == 0 {
        return;
    }

    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };

    let _ = writeln!(screen, "\n--- {} debug ---", target);
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            continue;
        }
        let Some((line_target, msg)) = raw_line.split_once('|') else {
            continue;
        };
        if line_target != target {
            continue;
        }

        if highlight(msg) {
            screen.set_color(Color::LightGreen);
        } else {
            screen.set_color(Color::White);
        }
        let _ = writeln!(screen, "{}", msg);
    }

    screen.set_color(Color::White);
    if overflow {
        let _ = writeln!(screen, "[... log output truncated ...]");
    }
    let _ = writeln!(screen, "--- end {} debug ---", target);
}
