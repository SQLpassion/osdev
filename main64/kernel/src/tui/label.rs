//! Label widget
//!
//! A `Label` occupies a single row and renders a fixed text string with a
//! configurable foreground and background color.  It is typically used as a
//! title bar or status line at the top or bottom of the screen.

use crate::drivers::screen::{Color, with_screen};
use crate::tui::{SCREEN_COLS, SCREEN_ROWS};

/// A single-line static text widget.
pub struct Label {
    /// Zero-based screen row.
    row: usize,
    /// Zero-based starting column.
    col: usize,
    /// Total width in columns (text is padded / clipped to this width).
    width: usize,
    /// Text content to display (printable ASCII only).
    text: &'static str,
    /// Foreground color.
    fg: Color,
    /// Background color.
    bg: Color,
}

impl Label {
    /// Construct a new `Label`.
    ///
    /// `width` controls how many columns the label occupies; the text is
    /// left-aligned and the remainder is filled with spaces so the background
    /// color spans the full widget width.
    pub const fn new(
        row: usize,
        col: usize,
        width: usize,
        text: &'static str,
        fg: Color,
        bg: Color,
    ) -> Self {
        Self { row, col, width, text, fg, bg }
    }

    /// Render the label into the VGA buffer.
    ///
    /// The label fills its entire width with spaces first (so the background
    /// color is solid), then overlays the text string from the left edge.
    pub fn draw(&self) {
        // Guard: skip silently if the label's row is off-screen.
        if self.row >= SCREEN_ROWS {
            return;
        }

        // Clamp draw width to the available screen width.
        let draw_width = self.width.min(SCREEN_COLS.saturating_sub(self.col));

        with_screen(|screen| {
            // Step 1: fill the entire widget area with the background color so
            //         no stale content from a previous frame bleeds through.
            screen.fill_rect(self.row, self.col, draw_width, 1, b' ', self.fg, self.bg);

            // Step 2: overlay the text string; `draw_at` clips at the right
            //         edge so no bounds check is needed here.
            screen.draw_at(self.row, self.col, self.text, self.fg, self.bg);
        });
    }
}
