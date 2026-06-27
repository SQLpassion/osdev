//! Active Console Implementation Router.
//!
//! Provides the `ConsoleImpl` enum wrapper that delegates all `KernelConsole`
//! operations to the currently active backend.

use super::{FramebufferConsole, KernelConsole, VgaConsole};
use crate::drivers::screen::Color;

/// Active Console Implementation Wrapper.
///
/// An enum-based dispatch pattern (or "Enum Dispatch") that acts as a container
/// for the active backend. While `with_console` still returns a `&mut dyn KernelConsole`
/// for API simplicity, this enum avoids heap allocating `Box<dyn KernelConsole>`
/// internally.
pub enum ConsoleImpl {
    Vga(VgaConsole),
    Framebuffer(FramebufferConsole),
}

impl core::fmt::Write for ConsoleImpl {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Delegate string formatting directly to the active variant.
        match self {
            ConsoleImpl::Vga(inner) => inner.write_str(s),
            ConsoleImpl::Framebuffer(inner) => inner.write_str(s),
        }
    }
}

impl KernelConsole for ConsoleImpl {
    fn clear(&mut self) {
        // Clear screen and reset cursor on the active console.
        match self {
            ConsoleImpl::Vga(inner) => inner.clear(),
            ConsoleImpl::Framebuffer(inner) => inner.clear(),
        }
    }

    fn print_char(&mut self, c: u8) {
        // Write character and update cursor layout.
        match self {
            ConsoleImpl::Vga(inner) => inner.print_char(c),
            ConsoleImpl::Framebuffer(inner) => inner.print_char(c),
        }
    }

    fn print_str(&mut self, s: &str) {
        // Write string in batch mode.
        match self {
            ConsoleImpl::Vga(inner) => inner.print_str(s),
            ConsoleImpl::Framebuffer(inner) => inner.print_str(s),
        }
    }

    fn set_color(&mut self, color: Color) {
        // Update printing color.
        match self {
            ConsoleImpl::Vga(inner) => inner.set_color(color),
            ConsoleImpl::Framebuffer(inner) => inner.set_color(color),
        }
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        // Relocate hardware/software cursor position.
        match self {
            ConsoleImpl::Vga(inner) => inner.set_cursor(row, col),
            ConsoleImpl::Framebuffer(inner) => inner.set_cursor(row, col),
        }
    }

    fn get_cursor(&self) -> (usize, usize) {
        // Read current cursor coordinate offset.
        match self {
            ConsoleImpl::Vga(inner) => inner.get_cursor(),
            ConsoleImpl::Framebuffer(inner) => inner.get_cursor(),
        }
    }

    fn draw_box(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        fg: Color,
        bg: Color,
    ) {
        // Paint CP437 box frame coordinates.
        match self {
            ConsoleImpl::Vga(inner) => inner.draw_box(row, col, width, height, fg, bg),
            ConsoleImpl::Framebuffer(inner) => inner.draw_box(row, col, width, height, fg, bg),
        }
    }

    fn draw_at(&mut self, row: usize, col: usize, text: &str, fg: Color, bg: Color) {
        // Draw string at raw absolute position.
        match self {
            ConsoleImpl::Vga(inner) => inner.draw_at(row, col, text, fg, bg),
            ConsoleImpl::Framebuffer(inner) => inner.draw_at(row, col, text, fg, bg),
        }
    }

    fn fill_rect(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        ch: u8,
        fg: Color,
        bg: Color,
    ) {
        // Fill block region.
        match self {
            ConsoleImpl::Vga(inner) => inner.fill_rect(row, col, width, height, ch, fg, bg),
            ConsoleImpl::Framebuffer(inner) => inner.fill_rect(row, col, width, height, ch, fg, bg),
        }
    }

    fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        // Draw single char at raw absolute position.
        match self {
            ConsoleImpl::Vga(inner) => inner.draw_char_at(row, col, ch, fg, bg),
            ConsoleImpl::Framebuffer(inner) => inner.draw_char_at(row, col, ch, fg, bg),
        }
    }

    fn blit_framebuffer(&mut self, cells: &[u16]) {
        // Batch write text cells grid.
        match self {
            ConsoleImpl::Vga(inner) => inner.blit_framebuffer(cells),
            ConsoleImpl::Framebuffer(inner) => inner.blit_framebuffer(cells),
        }
    }

    fn get_dimensions(&self) -> (usize, usize) {
        match self {
            ConsoleImpl::Vga(inner) => inner.get_dimensions(),
            ConsoleImpl::Framebuffer(inner) => inner.get_dimensions(),
        }
    }

    fn disable_hw_cursor(&mut self) {
        // Disable hardware cursor.
        match self {
            ConsoleImpl::Vga(inner) => inner.disable_hw_cursor(),
            ConsoleImpl::Framebuffer(inner) => inner.disable_hw_cursor(),
        }
    }

    fn enable_hw_cursor(&mut self) {
        // Enable hardware cursor.
        match self {
            ConsoleImpl::Vga(inner) => inner.enable_hw_cursor(),
            ConsoleImpl::Framebuffer(inner) => inner.enable_hw_cursor(),
        }
    }

    fn disable_blink_mode(&mut self) {
        // Disable blinking text mode.
        match self {
            ConsoleImpl::Vga(inner) => inner.disable_blink_mode(),
            ConsoleImpl::Framebuffer(inner) => inner.disable_blink_mode(),
        }
    }

    fn enable_blink_mode(&mut self) {
        // Re-enable blinking text mode.
        match self {
            ConsoleImpl::Vga(inner) => inner.enable_blink_mode(),
            ConsoleImpl::Framebuffer(inner) => inner.enable_blink_mode(),
        }
    }
}
