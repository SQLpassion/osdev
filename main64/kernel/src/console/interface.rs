//! Kernel Console Interface and Routing.
//!
//! Provides the primary abstraction trait `KernelConsole` along with the global
//! routing state and helper functions to dispatch text output dynamically to
//! either a VGA text buffer or a graphics framebuffer.

#![allow(clippy::too_many_arguments)]

use super::{ConsoleImpl, FramebufferConsole, VgaConsole};
use crate::boot_info::VideoModeType;
use crate::drivers::screen::Color;
use crate::sync::spinlock::SpinLock;

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
    fn disable_blink_mode(&mut self);

    /// Restores default VGA text mode blinking behavior.
    fn enable_blink_mode(&mut self);
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
pub fn with_console<R>(f: impl FnOnce(&mut dyn KernelConsole) -> R) -> R {
    // Step 1: Acquire the spinlock to gain exclusive access and disable interrupts.
    let mut guard = GLOBAL_CONSOLE.lock();

    // Step 2: Unwrap the option. Guaranteed to succeed as it is default-initialized.
    let console = guard
        .as_mut()
        .expect("GLOBAL_CONSOLE has not been initialized!");

    // Step 3: Run the user closure against the active console implementation.
    f(console)
}
