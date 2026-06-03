//! TextBox widget
//!
//! A multi-line static text display surrounded by a CP437 single-line box
//! border.  Lines are left-aligned within the inner area; content that
//! exceeds the inner width is silently clipped by `Screen::draw_at`.
//!
//! Layout (width=80, height=10):
//! ```
//! ┌──────────────────────────────────────────────────────────────────────────────┐
//! │ First line of text                                                           │
//! │ Second line of text                                                          │
//! │ ...                                                                          │
//! └──────────────────────────────────────────────────────────────────────────────┘
//! ```

extern crate alloc;

use alloc::vec::Vec;
use crate::drivers::screen::{Color, with_screen};
use crate::tui::{SCREEN_COLS, SCREEN_ROWS};

/// A multi-line static text widget surrounded by a box border.
pub struct TextBox {
    /// Zero-based screen row of the top-left corner (outer border).
    row: usize,
    /// Zero-based screen column of the top-left corner.
    col: usize,
    /// Total outer width (including the 1-column border on each side).
    width: usize,
    /// Total outer height (including the 1-row border at top and bottom).
    height: usize,
    /// Dynamic backing array of text lines (`&'static str`).
    lines: Vec<&'static str>,
    /// Foreground color for the text content.
    fg: Color,
    /// Background color filling the interior.
    bg: Color,
    /// Color used for the box border characters.
    border_fg: Color,
}

impl TextBox {
    /// Construct a new `TextBox` from a slice of static string references.
    ///
    /// Lines that exceed the inner width are clipped at render time by `Screen::draw_at`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        lines: &[&'static str],
        fg: Color,
        bg: Color,
        border_fg: Color,
    ) -> Self {
        Self {
            row,
            col,
            width,
            height,
            lines: Vec::from(lines),
            fg,
            bg,
            border_fg,
        }
    }

    /// Render the text box into the VGA buffer.
    ///
    /// Step 1: Draw the outer box border (single-line CP437 characters).
    /// Step 2: Fill the entire interior with the background color so no stale
    ///         content from a previous frame bleeds through.
    /// Step 3: Draw each text line from the top of the interior downward.
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS {
            return;
        }

        with_screen(|screen| {
            // Step 1: outer border frame.
            screen.draw_box(self.row, self.col, self.width, self.height, self.border_fg, self.bg);

            // Step 2: clear interior so no ghost characters remain.
            let inner_width = self.width.saturating_sub(2);
            let inner_height = self.height.saturating_sub(2);
            screen.fill_rect(
                self.row + 1,
                self.col + 1,
                inner_width,
                inner_height,
                b' ',
                self.fg,
                self.bg,
            );

            // Step 3: render each text line; clip at inner_height.
            for (i, &line) in self.lines.iter().enumerate() {
                if i >= inner_height {
                    break;
                }
                // One column of padding inside the border.
                screen.draw_at(self.row + 1 + i, self.col + 1, line, self.fg, self.bg);
            }
        });
    }
}
