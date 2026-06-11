//! ProgressBar widget
//!
//! Renders a horizontal fill bar with a percentage indicator.
//!
//! Visual layout (example: width=40, value=42):
//! `[████████████░░░░░░░░░░░░░░░░░]  42%`
//!
//! The bar inner width is `width - 7` (accounting for `[`, `]`, ` `, and the
//! three-character right-aligned percentage field plus `%`).

use crate::drivers::screen::{Color, with_screen};
use crate::tui::SCREEN_ROWS;

/// CP437 block character for the filled portion of the bar.
const FILL_CHAR: u8 = 0xDB; // █

/// CP437 light-shade character for the empty portion of the bar.
const EMPTY_CHAR: u8 = 0xB0; // ░

/// Minimum total widget width (must accommodate at least one bar cell).
const MIN_WIDTH: usize = 9;

/// A standalone horizontal progress bar widget.
///
/// Useful for showing a single metric without a text label, or as the
/// internal building block of the `Gauge` widget.
pub struct ProgressBar {
    /// Zero-based screen row.
    row: usize,
    /// Zero-based starting column.
    col: usize,
    /// Total widget width in columns.
    width: usize,
    /// Current fill value (clamped to 0..=100).
    value: usize,
    /// Foreground color (brackets, empty chars, percentage text).
    fg: Color,
    /// Background color.
    bg: Color,
    /// Color applied to the filled portion of the bar.
    fill_color: Color,
}

impl ProgressBar {
    /// Construct a new `ProgressBar`.
    ///
    /// `value` is clamped to `0..=100` at construction time.
    pub const fn new(
        row: usize,
        col: usize,
        width: usize,
        value: usize,
        fg: Color,
        bg: Color,
        fill_color: Color,
    ) -> Self {
        let v = if value > 100 { 100 } else { value };
        Self { row, col, width, value: v, fg, bg, fill_color }
    }

    /// Update the current fill value (0..=100).
    #[allow(dead_code)]
    pub fn set_value(&mut self, value: usize) {
        self.value = value.min(100);
    }

    /// Render the progress bar into the VGA buffer.
    ///
    /// Layout breakdown:
    /// - `[`           1 col
    /// - filled cells  bar_width * value / 100
    /// - empty cells   bar_width - filled
    /// - `]`           1 col
    /// - ` `           1 col
    /// - 3-char pct    3 cols (space-padded, right-aligned)
    /// - `%`           1 col
    ///
    /// Fixed overhead: 7 cols → bar_width = width - 7.
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS || self.width < MIN_WIDTH {
            return;
        }

        // Compute inner bar width after subtracting fixed overhead.
        let bar_width = self.width - 7;
        let filled    = (bar_width * self.value) / 100;

        with_screen(|screen| {
            let mut c = self.col;

            // Step 1: opening bracket.
            screen.draw_char_at(self.row, c, b'[', self.fg, self.bg);
            c += 1;

            // Step 2: filled portion (█).
            for _ in 0..filled {
                screen.draw_char_at(self.row, c, FILL_CHAR, self.fill_color, self.bg);
                c += 1;
            }

            // Step 3: empty portion (░).
            for _ in filled..bar_width {
                screen.draw_char_at(self.row, c, EMPTY_CHAR, self.fg, self.bg);
                c += 1;
            }

            // Step 4: closing bracket.
            screen.draw_char_at(self.row, c, b']', self.fg, self.bg);
            c += 1;

            // Step 5: separator space.
            screen.draw_char_at(self.row, c, b' ', self.fg, self.bg);
            c += 1;

            // Step 6: percentage digits — 3 chars, right-aligned, space-padded.
            let hundreds = (self.value / 100) as u8;
            let tens     = ((self.value / 10) % 10) as u8;
            let units    = (self.value % 10) as u8;

            // Hundreds digit (only visible for value == 100).
            let h_char = if hundreds > 0 { b'0' + hundreds } else { b' ' };
            screen.draw_char_at(self.row, c, h_char, self.fg, self.bg);
            c += 1;

            // Tens digit (blank for single-digit values).
            let t_char = if self.value >= 10 { b'0' + tens } else { b' ' };
            screen.draw_char_at(self.row, c, t_char, self.fg, self.bg);
            c += 1;

            // Units digit.
            screen.draw_char_at(self.row, c, b'0' + units, self.fg, self.bg);
            c += 1;

            // Percent sign.
            screen.draw_char_at(self.row, c, b'%', self.fg, self.bg);
        });
    }
}
