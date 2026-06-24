//! ProgressBar widget — horizontal fill bar with percentage.
//!
//! Visualizes completion metrics on a horizontal line with bracket borders,
//! block characters (`█`), and a numeric trailing percentage readout (e.g., ` 74%`).

use crate::screen::{Color, with_screen};
use crate::screen_rows;

/// Block character code for the filled section of the progress bar.
const FILL_CHAR:  u8 = 0xDB; // █
/// Shade character code for the empty section of the progress bar.
const EMPTY_CHAR: u8 = 0xB0; // ░
/// Minimum width required to draw the bar and the trailing percentage.
const MIN_WIDTH: usize = 9;

/// A percentage-based horizontal progress bar.
pub struct ProgressBar {
    /// Zero-based vertical screen row index.
    row: usize,
    /// Zero-based horizontal screen column index where the progress bar starts.
    col: usize,
    /// Total width of the progress bar in columns (including borders and percentage readout).
    width: usize,
    /// Completion percentage value (0 to 100).
    value: usize,
    /// Color for brackets, empty sections, and the percentage text.
    fg: Color,
    /// Background color behind the progress bar.
    bg: Color,
    /// Color of the filled block characters.
    fill_color: Color,
}

impl ProgressBar {
    /// Creates a new progress bar.
    ///
    /// # Arguments
    /// * `row` - Vertical coordinate on the screen.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total allocated width in columns. Must be >= 9 to render properly.
    /// * `value` - Initial value (clamped between 0 and 100).
    /// * `fg` - Foreground color.
    /// * `bg` - Background color.
    /// * `fill_color` - Color of the filled block characters.
    pub const fn new(row: usize, col: usize, width: usize, value: usize, fg: Color, bg: Color, fill_color: Color) -> Self {
        let v = if value > 100 { 100 } else { value };
        Self { row, col, width, value: v, fg, bg, fill_color }
    }

    /// Updates the progress bar value, clamping it to a maximum of 100%.
    #[allow(dead_code)]
    pub fn set_value(&mut self, value: usize) { self.value = value.min(100); }

    /// Renders the progress bar to the screen buffer.
    pub fn draw(&self) {
        // Step 1: Validate row index and width constraints.
        if self.row >= screen_rows() || self.width < MIN_WIDTH { return; }

        // Step 2: Compute bar graphics dimensions.
        // We subtract 7 columns for border brackets `[...]` and percentage readout ` XXX%`.
        let bar_width = self.width - 7;
        let filled = (bar_width * self.value) / 100;

        // Step 3: Draw each component to the screen.
        with_screen(|screen| {
            let mut c = self.col;

            // Draw opening bracket.
            screen.draw_char_at(self.row, c, b'[', self.fg, self.bg); c += 1;

            // Draw filled block sections.
            for _ in 0..filled {
                screen.draw_char_at(self.row, c, FILL_CHAR, self.fill_color, self.bg);
                c += 1;
            }

            // Draw empty shaded sections.
            for _ in filled..bar_width {
                screen.draw_char_at(self.row, c, EMPTY_CHAR, self.fg, self.bg);
                c += 1;
            }

            // Draw closing bracket and separation space.
            screen.draw_char_at(self.row, c, b']', self.fg, self.bg); c += 1;
            screen.draw_char_at(self.row, c, b' ', self.fg, self.bg); c += 1;

            // Deconstruct percentage into individual digits.
            let hundreds = (self.value / 100) as u8;
            let tens     = ((self.value / 10) % 10) as u8;
            let units    = (self.value % 10) as u8;

            // Draw digits with leading-space suppression for cleaner alignment.
            let h_char = if hundreds > 0 { b'0' + hundreds } else { b' ' };
            screen.draw_char_at(self.row, c, h_char, self.fg, self.bg); c += 1;

            let t_char = if self.value >= 10 { b'0' + tens } else { b' ' };
            screen.draw_char_at(self.row, c, t_char, self.fg, self.bg); c += 1;

            screen.draw_char_at(self.row, c, b'0' + units, self.fg, self.bg); c += 1;

            // Draw percentage symbol.
            screen.draw_char_at(self.row, c, b'%', self.fg, self.bg);
        });
    }
}
