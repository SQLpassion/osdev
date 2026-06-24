//! Gauge widget — horizontal progress bar with a prepended label descriptor.
//!
//! Combined representation of a static prefix text label and a standard fill bar,
//! used to report system metrics like memory allocation or CPU load.

use crate::screen::{Color, with_screen};
use crate::progressbar::ProgressBar;
use crate::screen_rows;

/// Labeled horizontal progress meter widget.
///
/// Integrates a static text label (left side) with a percentage-based progress bar
/// (right side). The label and progress bar are drawn on the same text-mode row.
pub struct Gauge {
    /// Zero-based vertical screen row index.
    row: usize,
    /// Zero-based horizontal screen column index where the label starts.
    col: usize,
    /// Total width of the gauge (including label and progress bar) in text columns.
    #[allow(dead_code)]
    width: usize,
    /// Static string text prefix drawn on the left.
    label: &'static str,
    /// Number of screen columns allocated for the label.
    label_width: usize,
    /// Text foreground color of the label.
    label_fg: Color,
    /// General background color of the label area.
    bg: Color,
    /// Inner progress bar component managing the fill percentage and graphics.
    bar: ProgressBar,
}

impl Gauge {
    /// Creates a new gauge widget.
    ///
    /// # Arguments
    /// * `row` - Vertical coordinate on the screen.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total width of the combined label and bar.
    /// * `label` - Text title to display.
    /// * `label_width` - Columns reserved for the title; the remainder goes to the progress bar.
    /// * `value` - Initial percentage value (0 to 100).
    /// * `label_fg` - Color of the label text.
    /// * `fill_color` - Color of the filled portion of the progress bar.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        row: usize,
        col: usize,
        width: usize,
        label: &'static str,
        label_width: usize,
        value: usize,
        label_fg: Color,
        fill_color: Color,
    ) -> Self {
        // Step 1: Calculate columns and starting position for the bar.
        let bar_col = col + label_width;
        let bar_width = width.saturating_sub(label_width);

        // Step 2: Construct self with the inner ProgressBar component.
        Self {
            row,
            col,
            width,
            label,
            label_width,
            label_fg,
            bg: Color::Black,
            bar: ProgressBar::new(
                row,
                bar_col,
                bar_width,
                value,
                Color::White,
                Color::Black,
                fill_color,
            ),
        }
    }

    /// Sets the progress bar metric value (0 to 100).
    #[allow(dead_code)]
    pub fn set_value(&mut self, value: usize) {
        self.bar.set_value(value);
    }

    /// Renders the label and the progress bar to the screen buffer.
    pub fn draw(&self) {
        // Step 1: Verify row index is within physical screen limits.
        if self.row >= screen_rows() {
            return;
        }

        // Step 2: Draw the label text and pad any remaining label columns.
        with_screen(|screen| {
            screen.fill_rect(self.row, self.col, self.label_width, 1, b' ', self.label_fg, self.bg);
            screen.draw_at(self.row, self.col, self.label, self.label_fg, self.bg);
        });

        // Step 3: Draw the inner progress bar.
        self.bar.draw();
    }
}
