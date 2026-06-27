//! Tabs widget — horizontal tab selection bar.
//!
//! Renders a row of horizontal tabs across the screen, allowing users to switch
//! between different views. Highlights the active tab with high-contrast colors.

extern crate alloc;
use crate::screen::{with_screen, Color};
use alloc::vec::Vec;

/// Color settings for the selected active tab label.
const ACTIVE_FG: Color = Color::Black;
const ACTIVE_BG: Color = Color::LightCyan;
/// Color settings for inactive tab labels.
const INACTIVE_FG: Color = Color::White;
const INACTIVE_BG: Color = Color::Blue;
/// Default background fill colors for the tab bar empty space.
const BAR_FG: Color = Color::White;
const BAR_BG: Color = Color::Black;

/// Horizontal tab-based navigation bar widget.
pub struct Tabs {
    /// Zero-based vertical screen row index.
    row: usize,
    /// Zero-based horizontal screen column index where the tab bar starts.
    col: usize,
    /// Total width of the tab bar in columns.
    width: usize,
    /// List of tab label strings.
    labels: Vec<&'static str>,
    /// Index of the currently active tab.
    active: usize,
}

impl Tabs {
    /// Creates a new empty tab selection bar.
    ///
    /// # Arguments
    /// * `row` - Vertical coordinate on the screen.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total allocated width in columns.
    pub fn new(row: usize, col: usize, width: usize) -> Self {
        Self {
            row,
            col,
            width,
            labels: Vec::new(),
            active: 0,
        }
    }

    /// Appends a new tab to the right side of the selection bar.
    pub fn add_tab(&mut self, label: &'static str) {
        self.labels.push(label);
    }

    /// Returns the index of the currently selected tab.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Shifts selection to the previous tab (left), clamping at index 0.
    pub fn select_prev(&mut self) {
        if self.active > 0 {
            self.active -= 1;
        }
    }

    /// Shifts selection to the next tab (right), clamping at the last item.
    pub fn select_next(&mut self) {
        if self.active + 1 < self.labels.len() {
            self.active += 1;
        }
    }

    /// Renders the tab selection bar to the screen buffer.
    pub fn draw(&self) {
        with_screen(|screen| {
            // Step 1: Draw the base background row for the entire tab bar width.
            screen.fill_rect(self.row, self.col, self.width, 1, b' ', BAR_FG, BAR_BG);
            let mut c = self.col + 1;

            // Step 2: Render each tab label with proper color highlight.
            for i in 0..self.labels.len() {
                // Bounds-check: stop rendering if we run out of column space.
                if c + 4 >= self.col + self.width {
                    break;
                }

                let label = self.labels[i];
                let (fg, bg) = if i == self.active {
                    (ACTIVE_FG, ACTIVE_BG)
                } else {
                    (INACTIVE_FG, INACTIVE_BG)
                };

                // Draw leading padding space.
                screen.draw_char_at(self.row, c, b' ', fg, bg);
                c += 1;
                // Draw tab text.
                screen.draw_at(self.row, c, label, fg, bg);
                c += label.len();
                // Draw trailing padding space.
                screen.draw_char_at(self.row, c, b' ', fg, bg);
                c += 1;

                // Draw delimiter spacing between tabs.
                if i + 1 < self.labels.len() {
                    screen.draw_char_at(self.row, c, b' ', BAR_FG, BAR_BG);
                    c += 1;
                }

                if c >= self.col + self.width {
                    break;
                }
            }
        });
    }
}
