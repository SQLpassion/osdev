//! Central kernel logging with optional in-memory capture for console dump.

use crate::sync::spinlock::SpinLock;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};

use crate::console::KernelConsole;
use crate::drivers::screen::Color;
use crate::drivers::serial;

const CAPTURE_BUF_SIZE: usize = 16 * 1024;

struct LogState {
    capture_enabled: bool,
    capture_len: usize,
    capture_overflow: bool,
    capture_buf: [u8; CAPTURE_BUF_SIZE],
}

/// Global logger with thread-safe access via SpinLock.
///
/// The lock disables interrupts during log capture to prevent race conditions
/// when multiple tasks attempt to write log messages concurrently.
struct GlobalLogger {
    inner: SpinLock<LogState>,
}

impl GlobalLogger {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(LogState {
                capture_enabled: false,
                capture_len: 0,
                capture_overflow: false,
                capture_buf: [0; CAPTURE_BUF_SIZE],
            }),
        }
    }
}

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

/// Executes a closure with mutable access to the logger state.
///
/// Thread-safe: acquires a spinlock that disables interrupts.
fn with_logger<R>(f: impl FnOnce(&mut LogState) -> R) -> R {
    let mut guard = LOGGER.inner.lock();
    f(&mut guard)
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
    logln_with_options(target, args, true, true);
}

/// Target-based log function with independent serial/capture control.
pub fn logln_with_options(
    target: &str,
    args: fmt::Arguments<'_>,
    serial_enabled: bool,
    capture_enabled: bool,
) {
    if serial_enabled {
        serial::_debug_print(format_args!("{}\n", args));
    }
    if capture_enabled {
        capture_target_line(target, args);
    }
}

/// Enable/disable capture buffer and reset it.
pub fn set_capture_enabled(enabled: bool) {
    with_logger(|state| {
        state.capture_enabled = enabled;
        state.capture_len = 0;
        state.capture_overflow = false;
    });
}

/// Copies the currently-valid bytes of the capture buffer (plus the overflow
/// flag) into an owned, heap-allocated buffer while still holding the logger
/// lock for the copy itself.
///
/// This is the fix for issue #54 ("`print_captured_target` reads the capture
/// buffer after releasing the logger lock"): the previous implementation only
/// snapshotted a `(ptr, len)` pair under the lock and then read *through*
/// that raw pointer after the lock was released. `capture_buf` is a
/// fixed-size array embedded in `LogState` and is never reallocated, so the
/// pointer itself stayed valid — but nothing prevented another task (woken
/// via a timer interrupt/preemption, since `capture_target_line` can run
/// concurrently again the moment the lock is free) from appending to, or
/// resetting via `set_capture_enabled`, that same live buffer while the slow,
/// multi-line screen-formatting loop was still reading it. That could garble
/// or truncate the dump, or make the UTF-8 decode fail outright.
///
/// The scratch buffer is allocated *before* the lock is taken, not inside the
/// locked closure: `heap::malloc`'s success path unconditionally logs an
/// `"alloc ptr=..."` line through this exact same logger (`with_logger`
/// again), and `SpinLock` is not reentrant, so allocating while already
/// holding `LOGGER.inner` would deadlock the CPU spinning against its own
/// held lock the instant `Vec` needed to grow. Doing the allocation first and
/// only `copy_from_slice`-ing (no allocation) under the lock keeps the
/// critical section both allocation-free and cheap — a bounded RAM-to-RAM
/// copy instead of the comparatively slow act of formatting and writing to
/// the screen — and guarantees callers read a self-consistent view
/// regardless of what happens to the live buffer afterward. This mirrors the
/// pattern already used for issue #16's deferred VRAM flush: snapshot the
/// cheap state under the lock, then perform the expensive part against the
/// owned copy, unlocked.
fn capture_snapshot() -> (Vec<u8>, bool) {
    // Step 1: Allocate the scratch buffer up front, before touching the
    // logger lock at all. Sized to the full capture capacity so the copy
    // below never needs to grow it (no reallocation, hence no further
    // allocator calls, while the lock from Step 2 is held).
    let mut scratch: Vec<u8> = alloc::vec![0u8; CAPTURE_BUF_SIZE];

    // Step 2: Copy only the currently-valid bytes under the lock. This is a
    // plain, allocation-free `copy_from_slice`, so the critical section stays
    // short and cannot recurse back into `with_logger`.
    let (len, overflow) = with_logger(|state| {
        scratch[..state.capture_len].copy_from_slice(&state.capture_buf[..state.capture_len]);
        (state.capture_len, state.capture_overflow)
    });

    // Step 3: Trim the scratch buffer down to the bytes that were valid at
    // snapshot time. `Vec::truncate` only drops the tail elements and updates
    // the length; for `u8` that is a no-op drop, so this never reallocates
    // and stays outside the lock.
    scratch.truncate(len);

    (scratch, overflow)
}

/// Test-only accessor for [`capture_snapshot`].
///
/// Exposed so integration tests can verify that formatting reads this owned
/// snapshot rather than the live capture buffer: a test can take a snapshot,
/// then mutate the live buffer (simulating a preempting task appending to it
/// or resetting it via `set_capture_enabled`), and confirm the
/// already-taken snapshot's contents remain unaffected (issue #54).
#[doc(hidden)]
pub fn capture_snapshot_for_test() -> (Vec<u8>, bool) {
    capture_snapshot()
}

/// Test-only accessor for [`format_captured_lines`].
///
/// Lets integration tests format a previously-taken snapshot (see
/// [`capture_snapshot_for_test`]) directly, so they can prove formatting
/// operates purely on that owned copy and is unaffected by later mutations
/// of the live capture buffer (issue #54).
#[doc(hidden)]
pub fn format_captured_for_test(
    bytes: &[u8],
    target: &str,
    overflow: bool,
    screen: &mut dyn KernelConsole,
    highlight: impl FnMut(&str) -> bool,
) {
    format_captured_lines(bytes, target, overflow, screen, highlight);
}

/// Formats previously-captured log lines for one `target` to `screen`,
/// highlighting lines for which `highlight` returns `true`.
///
/// Operates purely on the already-copied `bytes`/`overflow` snapshot and does
/// not touch `LOGGER` at all, so it is safe to call without holding the
/// logger lock (see `print_captured_target`, which takes the snapshot and
/// then calls this).
fn format_captured_lines(
    bytes: &[u8],
    target: &str,
    overflow: bool,
    screen: &mut dyn KernelConsole,
    mut highlight: impl FnMut(&str) -> bool,
) {
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

/// Dump captured logs for one target to the console.
pub fn print_captured_target(
    screen: &mut dyn KernelConsole,
    target: &str,
    highlight: impl FnMut(&str) -> bool,
) {
    // Step 1: Copy the buffer contents out from under the logger lock (see
    // `capture_snapshot` for why). This keeps the interrupt-disabling
    // critical section short and guarantees the formatting pass below reads
    // a self-consistent view even if capture stays enabled and another task
    // mutates the live buffer immediately afterward.
    let (bytes, overflow) = capture_snapshot();

    if bytes.is_empty() {
        return;
    }

    // Step 2: Format the owned snapshot. No lock is held here, so the
    // (comparatively slow) per-line screen writes below no longer keep
    // interrupts disabled for their entire duration.
    format_captured_lines(&bytes, target, overflow, screen, highlight);
}
