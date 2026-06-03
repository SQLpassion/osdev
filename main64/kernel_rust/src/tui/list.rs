//! List widget
//!
//! A scrollable, selectable list of text items rendered inside a single-line
//! box border.  The list supports keyboard navigation via the arrow keys and
//! highlights the currently selected item with an inverted color scheme.
//!
//! # Layout
//!
//! ```
//! row 0:  ┌───────────────────────────────────────┐   (box top)
//! row 1:  │  item 0 (may be selected)              │
//! row 2:  │  item 1                                │
//!  ...
//! row N:  └───────────────────────────────────────┘   (box bottom)
//! ```
//!
//! The interior height (`height - 2`) determines how many items are visible
//! at once.  When the selected item moves outside the visible window, the
//! scroll offset is adjusted so the selection is always on screen.

extern crate alloc;

use alloc::vec::Vec;
use crate::drivers::screen::{Color, with_screen};
use crate::tui::{SCREEN_COLS, SCREEN_ROWS};

/// Normal (unselected) item foreground color.
const ITEM_FG: Color = Color::White;

/// Normal (unselected) item background color.
const ITEM_BG: Color = Color::Black;

/// Selected item foreground color (inverted).
const SEL_FG: Color = Color::Black;

/// Selected item background color (inverted).
const SEL_BG: Color = Color::LightCyan;

/// Box border foreground color.
const BORDER_FG: Color = Color::LightCyan;

/// Box border background color.
const BORDER_BG: Color = Color::Black;

/// A scrollable, selectable list widget.
pub struct List {
    /// Top-left row of the outer box border.
    row: usize,
    /// Top-left column of the outer box border.
    col: usize,
    /// Total outer width (including the 1-column border on each side).
    width: usize,
    /// Total outer height (including the 1-row border at top and bottom).
    height: usize,
    /// Item strings stored in a dynamic Vec.
    items: Vec<&'static str>,
    /// Index of the currently highlighted item (0-based, absolute).
    selected: usize,
    /// First visible item index (scroll offset).
    scroll: usize,
}

impl List {
    /// Construct a new `List` from a slice of string references.
    pub fn new(
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        items: &[&'static str],
    ) -> Self {
        Self {
            row,
            col,
            width,
            height,
            items: Vec::from(items),
            selected: 0,
            scroll: 0,
        }
    }

    /// Number of item rows visible inside the box border.
    fn visible_rows(&self) -> usize {
        // The box uses one row for the top border and one for the bottom.
        self.height.saturating_sub(2)
    }

    /// Move the selection up by one item and scroll if needed.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;

            // Scroll up so the newly selected item stays in the visible window.
            if self.selected < self.scroll {
                self.scroll = self.selected;
            }
        }
    }

    /// Move the selection down by one item and scroll if needed.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;

            // Scroll down so the newly selected item stays in the visible window.
            let last_visible = self.scroll + self.visible_rows().saturating_sub(1);
            if self.selected > last_visible {
                self.scroll = self.selected - self.visible_rows().saturating_sub(1);
            }
        }
    }

    /// Return the index of the currently selected item.
    #[allow(dead_code)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Return the total number of items in the list.
    #[allow(dead_code)]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Render the list widget (border + all visible items) into the VGA buffer.
    pub fn draw(&self) {
        // Guard: nothing to draw if the widget is fully off-screen.
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS {
            return;
        }

        with_screen(|screen| {
            // Step 1: draw the outer box border.
            screen.draw_box(self.row, self.col, self.width, self.height, BORDER_FG, BORDER_BG);

            // Step 2: fill the entire interior with the default background so
            //         stale characters from a previous render are erased.
            let inner_col = self.col + 1;
            let inner_width = self.width.saturating_sub(2);
            let visible = self.visible_rows();
            screen.fill_rect(
                self.row + 1,
                inner_col,
                inner_width,
                visible,
                b' ',
                ITEM_FG,
                ITEM_BG,
            );

            // Step 3: render each visible item, highlighting the selected one.
            for vis_idx in 0..visible {
                let abs_idx = self.scroll + vis_idx;
                if abs_idx >= self.items.len() {
                    break;
                }

                let item_row = self.row + 1 + vis_idx;
                let is_selected = abs_idx == self.selected;

                // Choose inverted colors for the selected row.
                let (fg, bg) = if is_selected {
                    (SEL_FG, SEL_BG)
                } else {
                    (ITEM_FG, ITEM_BG)
                };

                // Fill the row background first so the highlight spans the
                // full inner width even for short item strings.
                screen.fill_rect(item_row, inner_col, inner_width, 1, b' ', fg, bg);

                // Overlay the item text; `draw_at` clips at the right edge.
                screen.draw_at(item_row, inner_col, self.items[abs_idx], fg, bg);
            }

            // Step 4: render a minimal scroll indicator in the bottom-right
            //         corner of the box so the user can see there is more content.
            //
            // Format: " 3/10 " — current (1-based) / total
            // We write this directly into the bottom border row to avoid
            // allocating a string buffer.
            let bottom_row = self.row + self.height - 1;
            let indicator_col = self.col + self.width.saturating_sub(8);
            
            let cur = self.selected + 1;           // 1-based
            let total = self.items.len();

            // We construct a scratch buffer for: " NN/NN "
            let mut buf = [b' '; 7];
            let mut pos = 6usize;                  // write from right to left

            // Write total (right-aligned).
            let mut n = total;
            loop {
                pos -= 1;
                buf[pos] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 { break; }
                if pos == 0 { break; }
            }

            // Insert separator.
            if pos > 0 {
                pos -= 1;
                buf[pos] = b'/';
            }

            // Write current (right-aligned, left of separator).
            let mut n = cur;
            loop {
                if pos == 0 { break; }
                pos -= 1;
                buf[pos] = b'0' + (n % 10) as u8;
                n /= 10;
                if n == 0 { break; }
            }

            // Draw the 7-byte indicator on the bottom border.
            for (i, &byte) in buf.iter().enumerate() {
                screen.draw_char_at(
                    bottom_row,
                    indicator_col + i,
                    byte,
                    BORDER_FG,
                    BORDER_BG,
                );
            }
        });
    }
}
