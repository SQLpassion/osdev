//! Graphics Framebuffer Console Implementation (Dummy).
//!
//! Implements a silent console backend that will render into the graphics GOP
//! framebuffer in the future.

use crate::drivers::screen::Color;
use super::KernelConsole;

/// Graphics GOP Framebuffer Console backend (Dummy).
///
/// A dummy implementation of the `KernelConsole` trait. This acts as a placeholder
/// for the GOP framebuffer console, which will be implemented in future phases.
pub struct FramebufferConsole;

impl core::fmt::Write for FramebufferConsole {
    fn write_str(&mut self, _s: &str) -> core::fmt::Result {
        // Dummy: ignore all string output for now.
        Ok(())
    }
}

impl KernelConsole for FramebufferConsole {
    fn clear(&mut self) {
        // Dummy: no-op.
    }

    fn print_char(&mut self, _c: u8) {
        // Dummy: no-op.
    }

    fn print_str(&mut self, _s: &str) {
        // Dummy: no-op.
    }

    fn set_color(&mut self, _color: Color) {
        // Dummy: no-op.
    }

    fn set_cursor(&mut self, _row: usize, _col: usize) {
        // Dummy: no-op.
    }

    fn get_cursor(&self) -> (usize, usize) {
        // Dummy: return origin coordinates.
        (0, 0)
    }

    fn draw_box(
        &mut self,
        _row: usize,
        _col: usize,
        _width: usize,
        _height: usize,
        _fg: Color,
        _bg: Color,
    ) {
        // Dummy: no-op.
    }

    fn draw_at(&mut self, _row: usize, _col: usize, _text: &str, _fg: Color, _bg: Color) {
        // Dummy: no-op.
    }

    fn fill_rect(
        &mut self,
        _row: usize,
        _col: usize,
        _width: usize,
        _height: usize,
        _ch: u8,
        _fg: Color,
        _bg: Color,
    ) {
        // Dummy: no-op.
    }

    fn draw_char_at(&mut self, _row: usize, _col: usize, _ch: u8, _fg: Color, _bg: Color) {
        // Dummy: no-op.
    }

    fn blit_framebuffer(&mut self, _cells: &[u16]) {
        // Dummy: no-op.
    }

    fn disable_hw_cursor(&mut self) {
        // Dummy: no-op.
    }

    fn enable_hw_cursor(&mut self) {
        // Dummy: no-op.
    }

    fn disable_blink_mode(&mut self) {
        // Dummy: no-op.
    }

    fn enable_blink_mode(&mut self) {
        // Dummy: no-op.
    }
}
