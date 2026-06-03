//! Table widget
//!
//! A scrollable, selectable data table with a header row, column separators,
//! and a box border drawn entirely from CP437 box-drawing characters.
//!
//! Layout (3 columns, 4 data rows, second row selected):
//! ```
//! ┌────────────────────────┬──────────────────────┬──────────────────────────┐
//! │ Region                 │ Address              │ Size / Type              │
//! ├────────────────────────┼──────────────────────┼──────────────────────────┤
//! │ Low Memory             │ 0x00000000           │ 640 KB | Usable          │
//! │ BIOS ROM Area          │ 0x000A0000           │ 384 KB | Reserved        │  ← selected
//! │ ...                    │ ...                  │ ...                      │
//! └────────────────────────┴──────────────────────┴──────────────────────────┘
//! ```
//!
//! # Height budget
//!
//! `height` includes the outer borders.  The visible data rows is therefore:
//! `height - 4`  (top border + header + separator + bottom border).

use crate::drivers::screen::{Color, Screen, with_screen};
use crate::tui::{SCREEN_COLS, SCREEN_ROWS};

/// Maximum number of columns a `Table` can hold.
pub const TABLE_MAX_COLS: usize = 6;

/// Maximum number of data rows a `Table` can hold without heap allocation.
pub const TABLE_MAX_ROWS: usize = 16;

/// Header row foreground color.
const HEADER_FG: Color = Color::Yellow;

/// Header row background color.
const HEADER_BG: Color = Color::Black;

/// Normal (unselected) data row foreground color.
const ROW_FG: Color = Color::White;

/// Normal data row background color.
const ROW_BG: Color = Color::Black;

/// Selected row foreground color.
const SEL_FG: Color = Color::Black;

/// Selected row background color.
const SEL_BG: Color = Color::LightCyan;

/// Box border foreground color.
const BORDER_FG: Color = Color::LightCyan;

/// Box border background color.
const BORDER_BG: Color = Color::Black;

// ---------------------------------------------------------------------------
// CP437 box-drawing byte constants
// ---------------------------------------------------------------------------
const TL: u8 = 0xDA; // ┌
const TR: u8 = 0xBF; // ┐
const BL: u8 = 0xC0; // └
const BR: u8 = 0xD9; // ┘
const H:  u8 = 0xC4; // ─
const V:  u8 = 0xB3; // │
const LT: u8 = 0xC3; // ├
const RT: u8 = 0xB4; // ┤
const CR: u8 = 0xC5; // ┼
const TT: u8 = 0xC2; // ┬
const BT: u8 = 0xC1; // ┴

/// A scrollable, selectable data table widget.
pub struct Table {
    /// Top-left row of the outer box border.
    row: usize,
    /// Top-left column of the outer box border.
    col: usize,
    /// Total outer width (including 1-column border on each side).
    width: usize,
    /// Total outer height (including top and bottom border rows).
    height: usize,
    /// Column header labels (up to `TABLE_MAX_COLS` entries).
    col_labels: [&'static str; TABLE_MAX_COLS],
    /// Content width of each column *excluding* the surrounding padding spaces
    /// and the vertical separator character.
    col_widths: [usize; TABLE_MAX_COLS],
    /// Number of populated columns.
    col_count: usize,
    /// Data rows stored in a fixed-size 2D array (`no_std`, no heap).
    rows: [[&'static str; TABLE_MAX_COLS]; TABLE_MAX_ROWS],
    /// Number of populated data rows.
    row_count: usize,
    /// Index of the currently selected row (0-based, absolute).
    selected: usize,
    /// Scroll offset: index of the first visible data row.
    scroll: usize,
}

impl Table {
    /// Construct a new `Table`.
    ///
    /// `col_labels` and `col_widths` must have the same length; the shorter
    /// slice determines `col_count`.  Pairs beyond `TABLE_MAX_COLS` are
    /// silently ignored.
    pub fn new(
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        col_labels: &[&'static str],
        col_widths: &[usize],
    ) -> Self {
        let col_count = col_labels.len().min(col_widths.len()).min(TABLE_MAX_COLS);

        let mut cl = [""; TABLE_MAX_COLS];
        let mut cw = [0usize; TABLE_MAX_COLS];
        for i in 0..col_count {
            cl[i] = col_labels[i];
            cw[i] = col_widths[i];
        }

        Self {
            row,
            col,
            width,
            height,
            col_labels: cl,
            col_widths: cw,
            col_count,
            rows: [[""; TABLE_MAX_COLS]; TABLE_MAX_ROWS],
            row_count: 0,
            selected: 0,
            scroll: 0,
        }
    }

    /// Append a data row.  Cells beyond `col_count` are ignored; rows beyond
    /// `TABLE_MAX_ROWS` are silently dropped.
    pub fn add_row(&mut self, cells: &[&'static str]) {
        if self.row_count >= TABLE_MAX_ROWS {
            return;
        }

        let n = cells.len().min(self.col_count);
        for i in 0..n {
            self.rows[self.row_count][i] = cells[i];
        }

        self.row_count += 1;
    }

    /// Move the selection up by one row, scrolling if necessary.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;

            // Scroll up so the newly selected row stays in the visible window.
            if self.selected < self.scroll {
                self.scroll = self.selected;
            }
        }
    }

    /// Move the selection down by one row, scrolling if necessary.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.row_count {
            self.selected += 1;

            // Scroll down so the newly selected row stays in the visible window.
            let visible = self.visible_rows();
            if self.selected >= self.scroll + visible {
                self.scroll = self.selected + 1 - visible;
            }
        }
    }

    /// Return the currently selected row index (0-based).
    #[allow(dead_code)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Return the total number of populated data rows.
    #[allow(dead_code)]
    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// Number of data rows that fit in the inner height of the widget.
    ///
    /// Budget: top border (1) + header (1) + separator (1) + bottom border (1) = 4.
    fn visible_rows(&self) -> usize {
        self.height.saturating_sub(4)
    }

    /// Render the full table widget into the VGA buffer.
    ///
    /// Step 1: Top border  ┌─┬─┐
    /// Step 2: Header row  │ col │ col │
    /// Step 3: Separator   ├─┼─┤
    /// Step 4: Data rows   │ cell │ cell │  (with selection highlight)
    /// Step 5: Empty rows  filled with background when fewer data rows
    /// Step 6: Bottom border └─┴─┘
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS {
            return;
        }

        with_screen(|screen| {
            // Step 1: top border.
            draw_h_border(screen, self.row, self.col, self.width, &self.col_widths, self.col_count, TL, TR, TT);

            // Step 2: header row.
            draw_row(
                screen,
                self.row + 1,
                self.col,
                self.width,
                &self.col_widths,
                self.col_count,
                &self.col_labels[..self.col_count],
                HEADER_FG,
                HEADER_BG,
            );

            // Step 3: separator between header and data.
            draw_h_border(screen, self.row + 2, self.col, self.width, &self.col_widths, self.col_count, LT, RT, CR);

            // Step 4 & 5: visible data rows (or blank rows when past end).
            let visible = self.visible_rows();
            for vis in 0..visible {
                let abs  = self.scroll + vis;
                let r    = self.row + 3 + vis;

                // Guard: do not draw past the bottom border row.
                if r >= self.row + self.height - 1 {
                    break;
                }

                if abs < self.row_count {
                    let (fg, bg) = if abs == self.selected {
                        (SEL_FG, SEL_BG)
                    } else {
                        (ROW_FG, ROW_BG)
                    };
                    draw_row(
                        screen,
                        r,
                        self.col,
                        self.width,
                        &self.col_widths,
                        self.col_count,
                        &self.rows[abs][..self.col_count],
                        fg,
                        bg,
                    );
                } else {
                    // Empty row: blank interior + side borders.
                    screen.fill_rect(r, self.col, self.width, 1, b' ', ROW_FG, ROW_BG);
                    screen.draw_char_at(r, self.col, V, BORDER_FG, BORDER_BG);
                    if self.col + self.width > 0 {
                        screen.draw_char_at(r, self.col + self.width - 1, V, BORDER_FG, BORDER_BG);
                    }
                }
            }

            // Step 6: bottom border.
            let bottom = self.row + self.height - 1;
            if bottom < SCREEN_ROWS {
                draw_h_border(screen, bottom, self.col, self.width, &self.col_widths, self.col_count, BL, BR, BT);
            }

            // Step 7: draw a scroll indicator in the bottom border.
            draw_scroll_indicator(screen, bottom, self.col + self.width, self.selected + 1, self.row_count);
        });
    }
}

// ---------------------------------------------------------------------------
// Module-level helper functions (avoid self-capture issues inside closures)
// ---------------------------------------------------------------------------

/// Draw a horizontal border row with corner and junction characters.
///
/// Layout:  `left` + (H ... H + `junc`) * col_count + `right`
fn draw_h_border(
    screen: &mut Screen,
    row: usize,
    col: usize,
    width: usize,
    col_widths: &[usize],
    col_count: usize,
    left: u8,
    right: u8,
    junc: u8,
) {
    if row >= SCREEN_ROWS { return; }

    let last_col = col + width - 1;

    // Left corner.
    screen.draw_char_at(row, col, left, BORDER_FG, BORDER_BG);

    let mut c = col + 1;

    for ci in 0..col_count {
        // Horizontal fill: 1 space + col_width + 1 space = col_width + 2 cells.
        let cell_width = col_widths[ci] + 2;
        for _ in 0..cell_width {
            if c >= last_col { break; }
            screen.draw_char_at(row, c, H, BORDER_FG, BORDER_BG);
            c += 1;
        }

        // Column junction (skip after the last column).
        if ci + 1 < col_count && c < last_col {
            screen.draw_char_at(row, c, junc, BORDER_FG, BORDER_BG);
            c += 1;
        }
    }

    // Fill any remaining space (e.g., rounding gaps) with horizontal lines.
    while c < last_col {
        screen.draw_char_at(row, c, H, BORDER_FG, BORDER_BG);
        c += 1;
    }

    // Right corner.
    screen.draw_char_at(row, last_col, right, BORDER_FG, BORDER_BG);
}

/// Draw a single data or header row with cell content and vertical separators.
///
/// Layout:  `│` + ` ` + text (clipped) + padding + ` ` + (`│` + ...) + `│`
fn draw_row(
    screen: &mut Screen,
    row: usize,
    col: usize,
    width: usize,
    col_widths: &[usize],
    col_count: usize,
    cells: &[&'static str],
    fg: Color,
    bg: Color,
) {
    if row >= SCREEN_ROWS { return; }

    let last_col = col + width - 1;

    // Left border.
    screen.draw_char_at(row, col, V, BORDER_FG, BORDER_BG);

    let mut c = col + 1;

    for ci in 0..col_count.min(cells.len()) {
        let cw = col_widths[ci];

        // One space of leading padding.
        if c < last_col {
            screen.draw_char_at(row, c, b' ', fg, bg);
            c += 1;
        }

        // Cell content area: fill with background, then overlay text.
        let available = cw.min(last_col.saturating_sub(c));
        screen.fill_rect(row, c, available, 1, b' ', fg, bg);
        let text = cells[ci];
        screen.draw_at(row, c, &text[..text.len().min(available)], fg, bg);
        c += available;

        // One space of trailing padding.
        if c < last_col {
            screen.draw_char_at(row, c, b' ', fg, bg);
            c += 1;
        }

        // Vertical column separator (skip after the last column).
        if ci + 1 < col_count && c < last_col {
            screen.draw_char_at(row, c, V, BORDER_FG, BORDER_BG);
            c += 1;
        }
    }

    // Blank-fill the remainder before the right border.
    while c < last_col {
        screen.draw_char_at(row, c, b' ', fg, bg);
        c += 1;
    }

    // Right border.
    if last_col < SCREEN_COLS {
        screen.draw_char_at(row, last_col, V, BORDER_FG, BORDER_BG);
    }
}

/// Write a compact `N/M` scroll indicator directly into the bottom border row,
/// just inside the right corner character.
fn draw_scroll_indicator(
    screen: &mut Screen,
    border_row: usize,
    right_border_col: usize,
    current: usize,
    total: usize,
) {
    if border_row >= SCREEN_ROWS { return; }

    // Maximum indicator width: " NN/NN " = 7 chars.
    let mut buf = [b' '; 7];
    let mut pos = 6usize;

    // Encode total (right-to-left).
    let mut n = total;
    loop {
        pos = pos.saturating_sub(1);
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 || pos == 0 { break; }
    }

    // Separator '/'.
    if pos > 0 {
        pos -= 1;
        buf[pos] = b'/';
    }

    // Encode current (right-to-left, stopping before the separator).
    let mut n = current;
    loop {
        if pos == 0 { break; }
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 { break; }
    }

    // Draw the indicator to the left of the right border character.
    let indicator_col = right_border_col.saturating_sub(buf.len() + 1);
    for (i, &byte) in buf.iter().enumerate() {
        let target_col = indicator_col + i;
        if target_col < SCREEN_COLS {
            screen.draw_char_at(border_row, target_col, byte, BORDER_FG, BORDER_BG);
        }
    }
}
