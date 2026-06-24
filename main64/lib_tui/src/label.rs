//! Label widget — single-line colored text.
//!
//! Provides a static single-line label element to display informational text,
//! headers, or descriptions with custom foreground and background colors.

use crate::screen::{Color, with_screen};
use crate::{screen_cols, screen_rows};

/// A single-line static text widget.
pub struct Label {
    /// Zero-based vertical screen row index.
    row: usize,
    /// Zero-based horizontal screen column index where the label starts.
    col: usize,
    /// Number of screen columns allocated for the label.
    width: usize,
    /// Static string text to display.
    text: &'static str,
    /// Text foreground color.
    fg: Color,
    /// Text background color.
    bg: Color,
}

impl Label {
    /// Creates a new label widget.
    ///
    /// # Arguments
    /// * `row` - Vertical coordinate on the screen.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total allocated width in columns.
    /// * `text` - Text content to display.
    /// * `fg` - Color of the text.
    /// * `bg` - Color of the background behind the text.
    pub const fn new(row: usize, col: usize, width: usize, text: &'static str, fg: Color, bg: Color) -> Self {
        Self { row, col, width, text, fg, bg }
    }

    /// Renders the label to the screen buffer.
    pub fn draw(&self) {
        // Step 1: Verify row index is within physical screen limits.
        if self.row >= screen_rows() { return; }

        // Step 2: Compute the maximum allowed width to prevent drawing past screen bounds.
        let draw_width = self.width.min(screen_cols().saturating_sub(self.col));

        // Step 3: Fill the background and draw the text.
        with_screen(|screen| {
            screen.fill_rect(self.row, self.col, draw_width, 1, b' ', self.fg, self.bg);
            screen.draw_at(self.row, self.col, self.text, self.fg, self.bg);
        });
    }
}
