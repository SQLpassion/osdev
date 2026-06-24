//! VGA Text-Mode Console Implementation.
//!
//! Delegates all draw and configuration operations to the VGA hardware driver.

use crate::drivers::screen::{Color, with_screen};
use super::KernelConsole;

/// VGA Text-Mode Console backend.
///
/// A zero-sized struct (ZST) implementing the `KernelConsole` trait by routing
/// all operations through the global screen interface helper `with_screen`.
pub struct VgaConsole;

impl core::fmt::Write for VgaConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Step 1: Lock the global screen lock and output the string.
        // `with_screen` disables interrupts to ensure atomicity.
        with_screen(|screen| screen.print_str(s));
        Ok(())
    }
}

impl KernelConsole for VgaConsole {
    fn clear(&mut self) {
        // Delegate clear screen operation to protected screen instance.
        with_screen(|screen| screen.clear());
    }

    fn print_char(&mut self, c: u8) {
        // Delegate write character operation.
        with_screen(|screen| screen.print_char(c));
    }

    fn print_str(&mut self, s: &str) {
        // Delegate print string operation.
        with_screen(|screen| screen.print_str(s));
    }

    fn set_color(&mut self, color: Color) {
        // Delegate foreground text color configuration.
        with_screen(|screen| screen.set_color(color));
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        // Delegate text-cursor repositioning.
        with_screen(|screen| screen.set_cursor(row, col));
    }

    fn get_cursor(&self) -> (usize, usize) {
        // Query text-cursor position.
        with_screen(|screen| screen.get_cursor())
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
        // Delegate CP437 box drawing border operation.
        with_screen(|screen| screen.draw_box(row, col, width, height, fg, bg));
    }

    fn draw_at(&mut self, row: usize, col: usize, text: &str, fg: Color, bg: Color) {
        // Delegate direct text drawing at coordinates.
        with_screen(|screen| screen.draw_at(row, col, text, fg, bg));
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
        // Delegate fill region operation.
        with_screen(|screen| screen.fill_rect(row, col, width, height, ch, fg, bg));
    }

    fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        // Delegate drawing single byte character.
        with_screen(|screen| screen.draw_char_at(row, col, ch, fg, bg));
    }

    fn blit_framebuffer(&mut self, cells: &[u16]) {
        // Delegate writing raw frame data.
        with_screen(|screen| screen.blit_framebuffer(cells));
    }

    fn get_dimensions(&self) -> (usize, usize) {
        // VGA text mode is strictly 80x25.
        (25, 80)
    }

    fn disable_hw_cursor(&mut self) {
        // Delegate hardware cursor hide operation.
        with_screen(|screen| screen.disable_hw_cursor());
    }

    fn enable_hw_cursor(&mut self) {
        // Delegate hardware cursor show/underline operation.
        with_screen(|screen| screen.enable_hw_cursor());
    }

    fn disable_blink_mode(&mut self) {
        // Delegate attribute mode disable-blink configuration.
        with_screen(|screen| screen.disable_blink_mode());
    }

    fn enable_blink_mode(&mut self) {
        // Delegate attribute mode enable-blink configuration.
        with_screen(|screen| screen.enable_blink_mode());
    }
}
