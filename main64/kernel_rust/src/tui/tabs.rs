//! Tabs widget
//!
//! A horizontal tab selection bar that renders up to `MAX_TABS` labeled tabs
//! and highlights the currently active one with inverted colors.
//!
//! Layout (4 tabs, tab 1 active):
//! ```
//!  [ Info ]  [ Memory ]  [ Devices ]  [ System ]
//! ```
//!
//! Left/Right arrow navigation is exposed via `select_prev` / `select_next`.

use crate::drivers::screen::{Color, with_screen};

/// Maximum number of tabs the bar can hold.
pub const MAX_TABS: usize = 8;

/// Foreground color for the active tab.
const ACTIVE_FG: Color = Color::Black;

/// Background color for the active tab (inverted highlight).
const ACTIVE_BG: Color = Color::LightCyan;

/// Foreground color for inactive tabs.
const INACTIVE_FG: Color = Color::White;

/// Background color for inactive tabs (must be < 8 to avoid VGA blink mode
/// on real hardware when disable_blink_mode() has not yet been called).
const INACTIVE_BG: Color = Color::Blue;

/// Color of the separator gaps between tabs.
const BAR_FG: Color = Color::White;

/// Background of the whole tab bar row.
const BAR_BG: Color = Color::Black;

/// A horizontal tab selection bar.
pub struct Tabs {
    /// Zero-based screen row.
    row: usize,
    /// Zero-based starting column.
    col: usize,
    /// Total width of the tab bar in columns.
    width: usize,
    /// Fixed-size array of tab label strings.
    labels: [&'static str; MAX_TABS],
    /// Number of populated tabs.
    tab_count: usize,
    /// Index of the currently active tab (0-based).
    active: usize,
}

impl Tabs {
    /// Construct an empty `Tabs` bar.  Call `add_tab` to populate it.
    pub const fn new(row: usize, col: usize, width: usize) -> Self {
        Self {
            row,
            col,
            width,
            labels: [""; MAX_TABS],
            tab_count: 0,
            active: 0,
        }
    }

    /// Append a tab with the given label.  Tabs beyond `MAX_TABS` are dropped.
    pub fn add_tab(&mut self, label: &'static str) {
        if self.tab_count < MAX_TABS {
            self.labels[self.tab_count] = label;
            self.tab_count += 1;
        }
    }

    /// Return the index of the currently active tab.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Move the active selection one tab to the left (wraps at 0).
    pub fn select_prev(&mut self) {
        if self.active > 0 {
            self.active -= 1;
        }
    }

    /// Move the active selection one tab to the right (clamps at last tab).
    pub fn select_next(&mut self) {
        if self.active + 1 < self.tab_count {
            self.active += 1;
        }
    }

    /// Render the tab bar into the VGA buffer.
    ///
    /// Step 1: Fill the entire bar row with the background color.
    /// Step 2: Draw each tab as `[ label ]` with a gap between tabs.
    ///         The active tab uses ACTIVE_FG / ACTIVE_BG colors; all others
    ///         use INACTIVE_FG / INACTIVE_BG.
    /// Step 3: Draw an underline character below active tab on next row
    ///         to visually separate it from content.
    pub fn draw(&self) {
        with_screen(|screen| {
            // Step 1: blank the full tab bar row.
            screen.fill_rect(self.row, self.col, self.width, 1, b' ', BAR_FG, BAR_BG);

            // Step 2: render each tab label with its colors.
            let mut c = self.col + 1;
            for i in 0..self.tab_count {
                if c + 4 >= self.col + self.width {
                    break;
                }

                let label = self.labels[i];
                let (fg, bg) = if i == self.active {
                    (ACTIVE_FG, ACTIVE_BG)
                } else {
                    (INACTIVE_FG, INACTIVE_BG)
                };

                // Draw: space + label + space  (three cell padding)
                screen.draw_char_at(self.row, c, b' ', fg, bg);
                c += 1;
                screen.draw_at(self.row, c, label, fg, bg);
                c += label.len();
                screen.draw_char_at(self.row, c, b' ', fg, bg);
                c += 1;

                // One blank gap between tabs (in bar colors).
                if i + 1 < self.tab_count {
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
