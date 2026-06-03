//! Gauge widget
//!
//! A labeled metric display that combines a fixed-width text label with a
//! `ProgressBar`.  The label is left-aligned and the bar fills the remainder
//! of the widget width.
//!
//! Visual layout (label_width=18, total width=78, value=60):
//! ```
//! Heap Memory:      [████████████████████░░░░░░░░░░░░░░░░░░░░]  60%
//! ```

use crate::drivers::screen::{Color, with_screen};
use crate::tui::progressbar::ProgressBar;
use crate::tui::SCREEN_ROWS;

/// A labeled progress gauge widget.
pub struct Gauge {
    /// Zero-based screen row.
    row: usize,
    /// Zero-based starting column.
    col: usize,
    /// Total widget width (label + bar).
    #[allow(dead_code)]
    width: usize,
    /// Static label text shown to the left of the bar.
    label: &'static str,
    /// Number of columns reserved for the label (text is clipped/padded here).
    label_width: usize,
    /// Foreground color for the label text.
    label_fg: Color,
    /// Background color for both label and bar area.
    bg: Color,
    /// The embedded progress bar that fills the remainder of the row.
    bar: ProgressBar,
}

impl Gauge {
    /// Construct a new `Gauge`.
    ///
    /// `label_width` must be smaller than `width` to leave room for the bar.
    /// `fill_color` is the color of the filled portion of the embedded bar.
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
        // The bar starts immediately after the label area.
        let bar_col   = col + label_width;
        let bar_width = width.saturating_sub(label_width);

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

    /// Update the current fill value (0..=100).
    #[allow(dead_code)]
    pub fn set_value(&mut self, value: usize) {
        self.bar.set_value(value);
    }

    /// Render the gauge into the VGA buffer.
    ///
    /// Step 1: blank the label area with the background color so no stale
    ///         text bleeds through.
    /// Step 2: draw the label text (clipped to `label_width`).
    /// Step 3: delegate to `ProgressBar::draw` for the bar + percentage.
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS {
            return;
        }

        with_screen(|screen| {
            // Step 1: clear the label area.
            screen.fill_rect(self.row, self.col, self.label_width, 1, b' ', self.label_fg, self.bg);

            // Step 2: overlay the label text.
            screen.draw_at(self.row, self.col, self.label, self.label_fg, self.bg);
        });

        // Step 3: the embedded bar handles its own draw region.
        self.bar.draw();
    }
}
