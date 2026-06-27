//! TextBox widget — multi-line static text inside a framed box border.
//!
//! Renders static paragraphs surrounded by a CP437 box frame. Clears its inner
//! area and draws each line of text aligned to the top-left of the box interior.

extern crate alloc;
use crate::screen::{with_screen, Color};
use crate::{screen_cols, screen_rows};
use alloc::vec::Vec;

/// Framed multi-line text container widget.
pub struct TextBox {
    /// Zero-based vertical screen row index where the top border starts.
    row: usize,
    /// Zero-based horizontal screen column index where the left border starts.
    col: usize,
    /// Total width of the text box (including borders) in columns.
    width: usize,
    /// Total height of the text box (including borders) in rows.
    height: usize,
    /// Vector of static text lines to display.
    lines: Vec<&'static str>,
    /// Text foreground color.
    fg: Color,
    /// Background color of the text box interior.
    bg: Color,
    /// Border frame color.
    border_fg: Color,
}

impl TextBox {
    /// Creates a new TextBox widget.
    ///
    /// # Arguments
    /// * `row` - Starting vertical coordinate.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total width in columns.
    /// * `height` - Total height in rows.
    /// * `lines` - Array of lines of text to display.
    /// * `fg` - Color of the text.
    /// * `bg` - Color of the background.
    /// * `border_fg` - Color of the surrounding frame borders.
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

    /// Renders the TextBox frame and its content lines to the screen buffer.
    pub fn draw(&self) {
        // Step 1: Validate starting coordinates.
        if self.row >= screen_rows() || self.col >= screen_cols() {
            return;
        }

        with_screen(|screen| {
            // Step 2: Draw the outer CP437 single-line box border frame.
            screen.draw_box(
                self.row,
                self.col,
                self.width,
                self.height,
                self.border_fg,
                self.bg,
            );

            // Step 3: Compute inner dimensions (excluding border columns/rows).
            let inner_width = self.width.saturating_sub(2);
            let inner_height = self.height.saturating_sub(2);

            // Step 4: Clear the interior area with empty spaces.
            screen.fill_rect(
                self.row + 1,
                self.col + 1,
                inner_width,
                inner_height,
                b' ',
                self.fg,
                self.bg,
            );

            // Step 5: Render text lines sequentially inside the cleared box.
            for (i, &line) in self.lines.iter().enumerate() {
                if i >= inner_height {
                    break;
                }
                screen.draw_at(self.row + 1 + i, self.col + 1, line, self.fg, self.bg);
            }
        });
    }
}
