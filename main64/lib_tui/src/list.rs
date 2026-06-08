//! List widget — scrollable selectable list container.
//!
//! Provides a framed menu box containing scrollable text items. Tracks the
//! selected index, handles scroll limits (sliding window viewport), and prints
//! a trailing layout pagination fraction (e.g., `3/12`) in the bottom border.

extern crate alloc;
use alloc::vec::Vec;
use crate::screen::{Color, with_screen};
use crate::{SCREEN_COLS, SCREEN_ROWS};

/// Default foreground color of inactive items.
const ITEM_FG:   Color = Color::White;
/// Default background color of inactive items.
const ITEM_BG:   Color = Color::Black;
/// High-contrast foreground color of the currently selected item.
const SEL_FG:    Color = Color::Black;
/// High-contrast background color of the currently selected item.
const SEL_BG:    Color = Color::LightCyan;
/// Border frame color.
const BORDER_FG: Color = Color::LightCyan;
/// Border background color.
const BORDER_BG: Color = Color::Black;

/// Framed scrollable item list selector widget.
pub struct List {
    /// Zero-based vertical screen row index where the top border starts.
    row: usize,
    /// Zero-based horizontal screen column index where the left border starts.
    col: usize,
    /// Total width of the list box (including borders) in columns.
    width: usize,
    /// Total height of the list box (including borders) in rows.
    height: usize,
    /// Vector of item strings.
    items: Vec<&'static str>,
    /// Index of the currently highlighted item.
    selected: usize,
    /// Index of the first visible item in the scrolling viewport.
    scroll: usize,
}

impl List {
    /// Creates a new List widget.
    ///
    /// # Arguments
    /// * `row` - Starting vertical coordinate.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total width.
    /// * `height` - Total height.
    /// * `items` - Array of menu item texts.
    pub fn new(row: usize, col: usize, width: usize, height: usize, items: &[&'static str]) -> Self {
        Self { row, col, width, height, items: Vec::from(items), selected: 0, scroll: 0 }
    }

    /// Computes the number of item rows visible inside the box frame.
    fn visible_rows(&self) -> usize { self.height.saturating_sub(2) }

    /// Decrements the selected index, adjusting the scroll window top if necessary.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            // Slide scroll window up if selection goes above the top visible item.
            if self.selected < self.scroll { self.scroll = self.selected; }
        }
    }

    /// Increments the selected index, adjusting the scroll window bottom if necessary.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
            let last_visible = self.scroll + self.visible_rows().saturating_sub(1);
            // Slide scroll window down if selection goes below the bottom visible item.
            if self.selected > last_visible {
                self.scroll = self.selected - self.visible_rows().saturating_sub(1);
            }
        }
    }

    /// Returns the index of the selected item.
    #[allow(dead_code)]
    pub fn selected(&self) -> usize { self.selected }

    /// Returns the total count of items.
    #[allow(dead_code)]
    pub fn item_count(&self) -> usize { self.items.len() }

    /// Renders the list box, items, selection highlight, and scroll index to the screen.
    pub fn draw(&self) {
        // Step 1: Validate starting coordinates.
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS { return; }

        with_screen(|screen| {
            // Step 2: Draw the outer CP437 box frame.
            screen.draw_box(self.row, self.col, self.width, self.height, BORDER_FG, BORDER_BG);

            let inner_col   = self.col + 1;
            let inner_width = self.width.saturating_sub(2);
            let visible     = self.visible_rows();

            // Step 3: Clear the entire inner text area.
            screen.fill_rect(self.row + 1, inner_col, inner_width, visible, b' ', ITEM_FG, ITEM_BG);

            // Step 4: Render each item within the current scrolling viewport.
            for vis_idx in 0..visible {
                let abs_idx = self.scroll + vis_idx;
                if abs_idx >= self.items.len() { break; }

                let item_row   = self.row + 1 + vis_idx;
                let is_selected = abs_idx == self.selected;
                let (fg, bg) = if is_selected { (SEL_FG, SEL_BG) } else { (ITEM_FG, ITEM_BG) };

                // Draw highlighted background row and render item text.
                screen.fill_rect(item_row, inner_col, inner_width, 1, b' ', fg, bg);
                screen.draw_at(item_row, inner_col, self.items[abs_idx], fg, bg);
            }

            // Step 5: Format and draw the scroll fraction (e.g. "3/12") in the bottom-right frame.
            let bottom_row    = self.row + self.height - 1;
            let indicator_col = self.col + self.width.saturating_sub(8);
            let cur   = self.selected + 1;
            let total = self.items.len();

            let mut buf = [b' '; 7];
            let mut pos = 6usize;
            let mut n = total;

            // Convert total count to characters.
            loop {
                pos -= 1; buf[pos] = b'0' + (n % 10) as u8; n /= 10;
                if n == 0 || pos == 0 { break; }
            }

            // Insert divisor separator.
            if pos > 0 { pos -= 1; buf[pos] = b'/'; }

            // Convert selected index count to characters.
            let mut n = cur;
            loop {
                if pos == 0 { break; }
                pos -= 1; buf[pos] = b'0' + (n % 10) as u8; n /= 10;
                if n == 0 { break; }
            }

            // Draw character block to the bottom border line.
            for (i, &byte) in buf.iter().enumerate() {
                screen.draw_char_at(bottom_row, indicator_col + i, byte, BORDER_FG, BORDER_BG);
            }
        });
    }
}
