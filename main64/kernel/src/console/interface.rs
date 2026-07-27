//! Kernel Console Interface and Routing.
//!
//! Provides the primary abstraction trait `KernelConsole` along with the global
//! routing state and helper functions to dispatch text output dynamically to
//! either a VGA text buffer or a graphics framebuffer.

#![allow(clippy::too_many_arguments)]

use super::{ConsoleImpl, FramebufferConsole, VgaConsole};
use crate::arch::interrupts;
use crate::boot_info::VideoModeType;
use crate::drivers::screen::Color;
use crate::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

/// Unified trait for kernel console outputs.
///
/// Any screen backend (e.g. text mode VGA, graphics pixel framebuffer) must
/// implement this trait. By extending `core::fmt::Write`, it integrates
/// natively with Rust's standard formatting macros (like `write!`/`writeln!`).
pub trait KernelConsole: core::fmt::Write + Send {
    /// Clears the screen and resets the cursor to the origin (0, 0).
    fn clear(&mut self);

    /// Prints a single ASCII character, handling control characters (like `\n`)
    /// and scrolling the screen if the character goes past the bottom boundary.
    fn print_char(&mut self, c: u8);

    /// Prints a full ASCII string. Updates the hardware cursor once at the end
    /// of the string to avoid costly per-character I/O port writes.
    fn print_str(&mut self, s: &str);

    /// Sets the text foreground color for subsequent print operations.
    fn set_color(&mut self, color: Color);

    /// Sets the cursor position to the specified coordinates (0-indexed).
    fn set_cursor(&mut self, row: usize, col: usize);

    /// Gets the current cursor position as a tuple `(row, col)`.
    fn get_cursor(&self) -> (usize, usize);

    /// Draws a boxed border (single-line CP437 box-drawing characters) at the
    /// specified rectangle, without changing the interior or advancing the cursor.
    fn draw_box(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        fg: Color,
        bg: Color,
    );

    /// Writes a string directly at the specified coordinate with custom colors,
    /// without advancing the cursor and without triggering scrolling.
    fn draw_at(&mut self, row: usize, col: usize, text: &str, fg: Color, bg: Color);

    /// Fills a rectangular region with a single character and explicit colors.
    /// Used primarily to clear background regions of TUI widgets before repainting.
    fn fill_rect(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        ch: u8,
        fg: Color,
        bg: Color,
    );

    /// Writes a single ASCII character directly at the specified coordinate
    /// with custom colors, bypassing cursor advancement.
    fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color);

    /// Blits a full raw text grid (typically 2000 cells) directly to the screen.
    fn blit_framebuffer(&mut self, cells: &[u16]);

    /// Gets the console dimensions as a tuple `(rows, cols)`.
    fn get_dimensions(&self) -> (usize, usize);

    /// Hides the hardware blink/text cursor.
    fn disable_hw_cursor(&mut self);

    /// Re-enables the hardware blink/text cursor.
    fn enable_hw_cursor(&mut self);

    /// Disables VGA blinking mode, enabling all 16 colors to be used as backgrounds.
    ///
    /// This toggles a VGA-attribute-controller-specific bit (the "blink vs.
    /// intense background" selector) and has no equivalent concept on
    /// non-VGA backends. Implementors that do not render through the VGA
    /// text-mode attribute controller (e.g. `FramebufferConsole`) MUST treat
    /// this as a silent no-op rather than erroring, since callers are not
    /// expected to branch on the active backend before calling it.
    fn disable_blink_mode(&mut self);

    /// Restores default VGA text mode blinking behavior.
    ///
    /// VGA-specific, like [`KernelConsole::disable_blink_mode`]; a no-op on
    /// backends that do not implement VGA-style attribute-controller
    /// blinking (e.g. `FramebufferConsole`).
    fn enable_blink_mode(&mut self);

    /// Extracts any VRAM upload queued by preceding drawing operations on this
    /// console, if the backend buffers pixel data in RAM instead of writing
    /// straight to hardware on every call (see `FramebufferConsole`).
    ///
    /// This step is intentionally kept cheap: implementors only snapshot the
    /// dirty pixel range into a private scratch buffer (a RAM-to-RAM `memcpy`)
    /// and reset their dirty tracking, so it remains safe to run while still
    /// holding `GLOBAL_CONSOLE`'s interrupt-disabling lock. The returned
    /// descriptor carries everything needed to perform the actual (slow,
    /// MMIO-bound) VRAM write, which `with_console` performs via
    /// [`PendingFlush::apply`] *after* releasing the lock. This is the fix for
    /// issue #16 ("Console holds an interrupt-disabling lock across
    /// full-screen VRAM flush"): the expensive blit no longer happens with
    /// interrupts disabled.
    ///
    /// Backends without a RAM backbuffer (e.g. `VgaConsole`, which writes
    /// each character straight to VGA text-mode MMIO as it goes and therefore
    /// has nothing to defer) rely on this default no-op implementation.
    fn take_pending_flush(&mut self) -> Option<PendingFlush> {
        None
    }
}

/// A deferred VRAM upload captured while `GLOBAL_CONSOLE`'s lock was held.
///
/// Produced by [`KernelConsole::take_pending_flush`] and applied by
/// `with_console` after the lock guard (and the interrupt-disabled window
/// that comes with holding it) has already been dropped. This keeps the
/// slow, MMIO-bound `copy_nonoverlapping` used to upload pixels to the
/// physical framebuffer out of the console's interrupt-masking critical
/// section — see issue #16.
///
/// # Accepted trade-off
///
/// Because the actual blit runs without the console lock held, a nested
/// caller (e.g. an interrupt handler that also prints to the console, see
/// `arch::interrupts::handlers`) could in principle start a *new* flush job
/// whose snapshot phase overlaps the scratch buffer this job is still
/// reading from. On this single-core kernel this can only manifest as a
/// visually inconsistent frame — never a use-after-free or an out-of-bounds
/// access, since the backing buffers are never reallocated for the lifetime
/// of the console backend. This is explicitly the trade-off called out by
/// issue #16 ("copy the dirty region out under the lock, then perform the
/// VRAM blit after releasing the lock") and is preferable to disabling
/// interrupts across a full-screen MMIO copy.
pub struct PendingFlush {
    src: *const u32,
    dst: *mut u32,
    len: usize,
}

impl PendingFlush {
    /// Builds a new pending flush descriptor.
    ///
    /// Only backends that override `take_pending_flush` (currently just
    /// `FramebufferConsole`) construct this. Keeping the fields private
    /// keeps the safety invariants documented on [`PendingFlush`] enforceable
    /// from a single place.
    pub(crate) fn new(src: *const u32, dst: *mut u32, len: usize) -> Self {
        Self { src, dst, len }
    }

    /// Performs the deferred VRAM write.
    ///
    /// Must be called *without* holding `GLOBAL_CONSOLE`'s lock (see
    /// `with_console`), so the slow MMIO copy does not happen with
    /// interrupts disabled.
    fn apply(self) {
        if self.len == 0 {
            return;
        }

        // Test-only introspection (see `last_flush_ran_with_interrupts_enabled`):
        // record whether interrupts were enabled right before performing the
        // (potentially large) VRAM copy below. Cheap enough to always record.
        LAST_FLUSH_INTERRUPTS_ENABLED.store(interrupts::are_enabled(), Ordering::Relaxed);

        // SAFETY:
        // - `src`/`dst`/`len` were validated against in-bounds, non-overlapping
        //   buffers at capture time by the backend that built this descriptor
        //   (see `FramebufferConsole::take_flush_job`).
        // - The scratch buffer backing `src` is heap-allocated once and never
        //   reallocated/moved for the lifetime of the console backend, and the
        //   physical framebuffer backing `dst` is fixed for the boot session,
        //   so both pointers remain valid even though the console lock that
        //   produced them has since been released.
        unsafe {
            core::ptr::copy_nonoverlapping(self.src, self.dst, self.len);
        }
    }
}

/// Test-only introspection flag recording whether interrupts were enabled at
/// the moment the most recent deferred VRAM flush (see [`PendingFlush::apply`])
/// actually ran. Read via [`last_flush_ran_with_interrupts_enabled`].
///
/// Used by integration tests to verify issue #16's fix: the (potentially
/// full-screen) VRAM `copy_nonoverlapping` must happen outside of
/// `GLOBAL_CONSOLE`'s interrupt-disabling critical section.
static LAST_FLUSH_INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(false);

/// Test-only accessor for [`LAST_FLUSH_INTERRUPTS_ENABLED`].
///
/// Follows the existing `#[doc(hidden)] pub` convention used elsewhere in the
/// kernel for test-only introspection hooks (e.g. `block::reset_active_device`,
/// `scheduler::reset_initialization_for_test`).
#[doc(hidden)]
pub fn last_flush_ran_with_interrupts_enabled() -> bool {
    LAST_FLUSH_INTERRUPTS_ENABLED.load(Ordering::Relaxed)
}

/// Global active console instance wrapper.
///
/// Default-initialized to the VGA text-mode driver to ensure immediate availability
/// during early boot and inside integration tests that bypass dynamic bootloader
/// structure parsing.
pub(crate) static GLOBAL_CONSOLE: SpinLock<Option<ConsoleImpl>> =
    SpinLock::new(Some(ConsoleImpl::Vga(VgaConsole)));

/// Initializes the dynamic console driver interface.
///
/// Should be called during early boot once the kernel is in possession of a
/// valid video mode structure (e.g. from BIOS VBE or UEFI/Linear Framebuffer).
pub fn init(video_type: VideoModeType) {
    // Step 1: Select the concrete backend driver corresponding to the boot mode.
    let console = match video_type {
        VideoModeType::VgaText => ConsoleImpl::Vga(VgaConsole),
        VideoModeType::Framebuffer => ConsoleImpl::Framebuffer(FramebufferConsole::new()),
    };

    // Step 2: Lock the global console and publish the active driver implementation.
    // This overrides the early-boot VGA default configuration.
    *GLOBAL_CONSOLE.lock() = Some(console);
}

/// Safely runs a closure with mutable access to the active kernel console.
///
/// Thread-safe: acquires the global spinlock that disables interrupts for the
/// duration of the closure. This prevents race conditions from concurrent logs
/// and preemption during screen draws.
///
/// Note on VRAM flushing (issue #16): the closure `f` may leave a deferred
/// VRAM upload queued (e.g. after a `FramebufferConsole` scroll or clear,
/// which can mark the entire screen dirty). Rather than performing that
/// upload from inside `f` — which would run the slow, MMIO-bound blit while
/// this function's lock still has interrupts disabled — we extract it via
/// [`KernelConsole::take_pending_flush`] (cheap, still under the lock) and
/// apply it only after the lock guard has been dropped.
pub fn with_console<R>(f: impl FnOnce(&mut dyn KernelConsole) -> R) -> R {
    // Step 1: Acquire the spinlock (disables interrupts) and run the closure,
    // then extract (but do not yet apply) any pending VRAM flush. Both of
    // these are cheap RAM-only operations, so it is fine to keep them inside
    // this critical section.
    let (result, pending_flush) = {
        let mut guard = GLOBAL_CONSOLE.lock();

        // Step 2: Unwrap the option. Guaranteed to succeed as it is default-initialized.
        let console = guard
            .as_mut()
            .expect("GLOBAL_CONSOLE has not been initialized!");

        // Step 3: Run the user closure against the active console implementation.
        let result = f(console);

        // Step 4: Snapshot any queued dirty region while still holding the lock.
        let pending_flush = console.take_pending_flush();

        (result, pending_flush)
        // `guard` is dropped here, releasing the lock and restoring interrupts
        // to whatever state they were in before this call.
    };

    // Step 5: Perform the actual (potentially large) VRAM upload, if any, now
    // that the lock has been released. This is the fix for issue #16: the
    // slow MMIO `copy_nonoverlapping` no longer runs with interrupts disabled.
    if let Some(flush) = pending_flush {
        flush.apply();
    }

    result
}
