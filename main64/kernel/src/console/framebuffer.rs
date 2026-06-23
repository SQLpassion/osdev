//! Graphics Framebuffer Console Implementation (Dummy).
//!
//! Implements a silent console backend that will render text into the graphics
//! framebuffer in the future.

use crate::drivers::screen::Color;
use super::KernelConsole;

/// Graphics Framebuffer Console backend (Dummy).
///
/// A dummy implementation of the `KernelConsole` trait. This acts as a placeholder
/// for the framebuffer console, which will be implemented in future phases.
///
/// When fully implemented, this driver will handle software text rendering by
/// drawing glyphs from a font bitmap directly into the linear physical framebuffer
/// allocated by the bootloader.
pub struct FramebufferConsole;

impl core::fmt::Write for FramebufferConsole {
    /// Writes a string slice to the console.
    ///
    /// Since the framebuffer console is currently a placeholder, it silently discards
    /// all input and returns `Ok(())`.
    fn write_str(&mut self, _s: &str) -> core::fmt::Result {
        // Dummy: ignore all string output for now.
        Ok(())
    }
}

impl KernelConsole for FramebufferConsole {
    /// Clears the screen.
    ///
    /// When implemented, this will fill the entire graphics framebuffer memory
    /// area with the current background color.
    fn clear(&mut self) {
        // Dummy: no-op.
    }

    /// Prints a single character to the screen at the current cursor position.
    ///
    /// When implemented, this will render the corresponding ASCII character glyph from
    /// the font bitmap and advance the text cursor.
    fn print_char(&mut self, _c: u8) {
        // Dummy: no-op.
    }

    /// Prints a string slice to the screen at the current cursor position.
    ///
    /// When implemented, this will iterate through the string, rendering each character,
    /// handling newlines (`\n`, `\r`), carriage returns, wrapping, and terminal scrolling.
    fn print_str(&mut self, _s: &str) {
        // Dummy: no-op.
    }

    /// Sets the text foreground and background colors.
    ///
    /// When implemented, this will update the active drawing colors used for text rendering.
    fn set_color(&mut self, _color: Color) {
        // Dummy: no-op.
    }

    /// Sets the text cursor coordinates.
    ///
    /// When implemented, this will update the internal cursor position (row, column).
    fn set_cursor(&mut self, _row: usize, _col: usize) {
        // Dummy: no-op.
    }

    /// Gets the current text cursor coordinates.
    ///
    /// Currently returns the origin (0, 0).
    fn get_cursor(&self) -> (usize, usize) {
        // Dummy: return origin coordinates.
        (0, 0)
    }

    /// Draws a double-line framed box on the screen.
    ///
    /// When implemented, this will render standard box-drawing borders and fill
    /// the interior area with the active background color.
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

    /// Draws a string starting at the specified coordinates.
    ///
    /// When implemented, this will temporarily override the cursor position and
    /// colors to print a string, resetting them afterwards.
    fn draw_at(&mut self, _row: usize, _col: usize, _text: &str, _fg: Color, _bg: Color) {
        // Dummy: no-op.
    }

    /// Fills a rectangular grid area with a specific character.
    ///
    /// When implemented, this will clear or fill the specified text cells area.
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

    /// Draws a single character at the specified coordinates.
    ///
    /// When implemented, this will render a character glyph at the target grid position.
    fn draw_char_at(&mut self, _row: usize, _col: usize, _ch: u8, _fg: Color, _bg: Color) {
        // Dummy: no-op.
    }

    /// Blits an array of text cells directly to the graphics framebuffer.
    ///
    /// When implemented, this will translate a VGA-compatible 16-bit text cell array
    /// (character byte + attribute byte) and render them into pixels on screen.
    fn blit_framebuffer(&mut self, _cells: &[u16]) {
        // Dummy: no-op.
    }

    /// Disables the visual rendering of the text cursor.
    ///
    /// When implemented, this will hide the blinking text cursor.
    fn disable_hw_cursor(&mut self) {
        // Dummy: no-op.
    }

    /// Enables the visual rendering of the text cursor.
    ///
    /// When implemented, this will show the blinking text cursor at the active coordinates.
    fn enable_hw_cursor(&mut self) {
        // Dummy: no-op.
    }

    /// Disables the cursor blinking behavior.
    ///
    /// When implemented, this will keep the cursor statically visible without blinking.
    fn disable_blink_mode(&mut self) {
        // Dummy: no-op.
    }

    /// Enables the cursor blinking behavior.
    ///
    /// When implemented, this will start the software cursor blinking cycle.
    fn enable_blink_mode(&mut self) {
        // Dummy: no-op.
    }
}
