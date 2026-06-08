//! Table widget — scrollable multi-column data grid.
//!
//! Renders a formatted data grid complete with column headers, column-spanning borders,
//! and vertical separators using CP437 table-drawing characters (such as ┌, ─, ┼, ┤, etc.).
//! Manages scroll limits and renders pagination details (e.g. "3/8") in the bottom-right corner.

extern crate alloc;
use alloc::vec::Vec;
use crate::screen::{Color, Screen, with_screen};
use crate::{SCREEN_COLS, SCREEN_ROWS};

/// Color settings for table column header labels.
const HEADER_FG: Color = Color::Yellow;
const HEADER_BG: Color = Color::Black;
/// Color settings for normal inactive data rows.
const ROW_FG:    Color = Color::White;
const ROW_BG:    Color = Color::Black;
/// Highlight colors for the currently selected data row.
const SEL_FG:    Color = Color::Black;
const SEL_BG:    Color = Color::LightCyan;
/// Default colors for the border grid line segments.
const BORDER_FG: Color = Color::LightCyan;
const BORDER_BG: Color = Color::Black;

// CP437 single-line table drawing symbols.
const TL: u8 = 0xDA; // Top-Left corner ┌
const TR: u8 = 0xBF; // Top-Right corner ┐
const BL: u8 = 0xC0; // Bottom-Left corner └
const BR: u8 = 0xD9; // Bottom-Right corner ┘
const H:  u8 = 0xC4; // Horizontal line ─
const V:  u8 = 0xB3; // Vertical line │
const LT: u8 = 0xC3; // Left T-junction ├
const RT: u8 = 0xB4; // Right T-junction ┤
const CR: u8 = 0xC5; // Cross junction ┼
const TT: u8 = 0xC2; // Top T-junction ┬
const BT: u8 = 0xC1; // Bottom T-junction ┴

/// A scrollable multi-column data grid widget.
pub struct Table {
    /// Zero-based vertical screen row index where the top border starts.
    row: usize,
    /// Zero-based horizontal screen column index where the left border starts.
    col: usize,
    /// Total width of the table box (including outer borders) in columns.
    width: usize,
    /// Total height of the table box (including headers and borders) in rows.
    height: usize,
    /// Column labels shown in the header row.
    col_labels: Vec<&'static str>,
    /// Column widths (excluding margins/borders) in character columns.
    col_widths: Vec<usize>,
    /// Grid cells containing the raw static text lines.
    rows: Vec<Vec<&'static str>>,
    /// Selected row index.
    selected: usize,
    /// Index of the first visible data row in the scroll viewport.
    scroll: usize,
}

impl Table {
    /// Creates a new empty Table widget.
    ///
    /// # Arguments
    /// * `row` - Vertical coordinate.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total width.
    /// * `height` - Total height.
    /// * `col_labels` - Names of the table columns.
    /// * `col_widths` - Width (in columns) for each corresponding column.
    pub fn new(row: usize, col: usize, width: usize, height: usize, col_labels: &[&'static str], col_widths: &[usize]) -> Self {
        let count = col_labels.len().min(col_widths.len());
        Self {
            row, col, width, height,
            col_labels: Vec::from(&col_labels[..count]),
            col_widths: Vec::from(&col_widths[..count]),
            rows: Vec::new(), selected: 0, scroll: 0,
        }
    }

    /// Appends a new data row. Auto-pads cells with empty strings if too short.
    pub fn add_row(&mut self, cells: &[&'static str]) {
        let mut row = Vec::new();
        let n = cells.len().min(self.col_labels.len());
        row.extend_from_slice(&cells[..n]);
        while row.len() < self.col_labels.len() { row.push(""); }
        self.rows.push(row);
    }

    /// Shifts selection up, adjusting the scroll window top if necessary.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll { self.scroll = self.selected; }
        }
    }

    /// Shifts selection down, adjusting the scroll window bottom if necessary.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.rows.len() {
            self.selected += 1;
            let visible = self.visible_rows();
            if self.selected >= self.scroll + visible {
                self.scroll = self.selected + 1 - visible;
            }
        }
    }

    /// Returns the index of the selected row.
    #[allow(dead_code)] pub fn selected(&self) -> usize { self.selected }
    /// Returns the total data row count.
    #[allow(dead_code)] pub fn row_count(&self) -> usize { self.rows.len() }

    /// Computes the number of visible data rows (excludes borders and header area).
    fn visible_rows(&self) -> usize { self.height.saturating_sub(4) }

    /// Renders the table layout, headers, content cells, and borders to the screen.
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS { return; }
        with_screen(|screen| {
            let col_count = self.col_labels.len();

            // Step 1: Draw top border line with T-junction joints.
            draw_h_border(screen, self.row, self.col, self.width, &self.col_widths, col_count, TL, TR, TT);

            // Step 2: Draw the column header text labels.
            draw_row(screen, self.row + 1, self.col, self.width, &self.col_widths, col_count, &self.col_labels, HEADER_FG, HEADER_BG);

            // Step 3: Draw header divider horizontal line with cross junctions.
            draw_h_border(screen, self.row + 2, self.col, self.width, &self.col_widths, col_count, LT, RT, CR);

            // Step 4: Draw visible data rows within the scroll window.
            let visible = self.visible_rows();
            for vis in 0..visible {
                let abs = self.scroll + vis;
                let r   = self.row + 3 + vis;
                if r >= self.row + self.height - 1 { break; }

                if abs < self.rows.len() {
                    let (fg, bg) = if abs == self.selected { (SEL_FG, SEL_BG) } else { (ROW_FG, ROW_BG) };
                    draw_row(screen, r, self.col, self.width, &self.col_widths, col_count, &self.rows[abs], fg, bg);
                } else {
                    // Fill empty trailing rows with spaces and draw side borders.
                    screen.fill_rect(r, self.col, self.width, 1, b' ', ROW_FG, ROW_BG);
                    screen.draw_char_at(r, self.col, V, BORDER_FG, BORDER_BG);
                    if self.col + self.width > 0 {
                        screen.draw_char_at(r, self.col + self.width - 1, V, BORDER_FG, BORDER_BG);
                    }
                }
            }

            // Step 5: Draw bottom border line with bottom T-junctions.
            let bottom = self.row + self.height - 1;
            if bottom < SCREEN_ROWS {
                draw_h_border(screen, bottom, self.col, self.width, &self.col_widths, col_count, BL, BR, BT);
            }

            // Step 6: Render bottom border scroll indicator fraction.
            draw_scroll_indicator(screen, bottom, self.col + self.width, self.selected + 1, self.rows.len());
        });
    }
}

/// Helper: draws a single horizontal border line dividing table sections.
#[allow(clippy::too_many_arguments)]
fn draw_h_border(screen: &mut Screen, row: usize, col: usize, width: usize, col_widths: &[usize], col_count: usize, left: u8, right: u8, junc: u8) {
    if row >= SCREEN_ROWS { return; }
    let last_col = col + width - 1;
    screen.draw_char_at(row, col, left, BORDER_FG, BORDER_BG);
    let mut c = col + 1;
    for (ci, &cw) in col_widths.iter().enumerate().take(col_count) {
        let cell_width = cw + 2; // Add padding spaces
        for _ in 0..cell_width {
            if c >= last_col { break; }
            screen.draw_char_at(row, c, H, BORDER_FG, BORDER_BG);
            c += 1;
        }
        if ci + 1 < col_count && c < last_col {
            screen.draw_char_at(row, c, junc, BORDER_FG, BORDER_BG);
            c += 1;
        }
    }
    while c < last_col { screen.draw_char_at(row, c, H, BORDER_FG, BORDER_BG); c += 1; }
    screen.draw_char_at(row, last_col, right, BORDER_FG, BORDER_BG);
}

/// Helper: draws a single row of column values flanked by vertical grid lines.
#[allow(clippy::too_many_arguments)]
fn draw_row(screen: &mut Screen, row: usize, col: usize, width: usize, col_widths: &[usize], col_count: usize, cells: &[&'static str], fg: Color, bg: Color) {
    if row >= SCREEN_ROWS { return; }
    let last_col = col + width - 1;
    screen.draw_char_at(row, col, V, BORDER_FG, BORDER_BG);
    let mut c = col + 1;
    for ci in 0..col_count.min(cells.len()) {
        let cw = col_widths[ci];
        if c < last_col { screen.draw_char_at(row, c, b' ', fg, bg); c += 1; }

        let available = cw.min(last_col.saturating_sub(c));
        screen.fill_rect(row, c, available, 1, b' ', fg, bg);
        let text = cells[ci];
        screen.draw_at(row, c, &text[..text.len().min(available)], fg, bg);
        c += available;

        if c < last_col { screen.draw_char_at(row, c, b' ', fg, bg); c += 1; }
        if ci + 1 < col_count && c < last_col {
            screen.draw_char_at(row, c, V, BORDER_FG, BORDER_BG);
            c += 1;
        }
    }
    while c < last_col { screen.draw_char_at(row, c, b' ', fg, bg); c += 1; }
    if last_col < SCREEN_COLS { screen.draw_char_at(row, last_col, V, BORDER_FG, BORDER_BG); }
}

/// Helper: draws the scroll fraction indicator into the bottom border.
fn draw_scroll_indicator(screen: &mut Screen, border_row: usize, right_border_col: usize, current: usize, total: usize) {
    if border_row >= SCREEN_ROWS { return; }
    let mut buf = [b' '; 7];
    let mut pos = 6usize;

    // Total rows fraction string.
    let mut n = total;
    loop {
        pos = pos.saturating_sub(1);
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 || pos == 0 { break; }
    }
    if pos > 0 { pos -= 1; buf[pos] = b'/'; }

    // Current selected row string.
    let mut n = current;
    loop {
        if pos == 0 { break; }
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 { break; }
    }

    let indicator_col = right_border_col.saturating_sub(buf.len() + 1);
    for (i, &byte) in buf.iter().enumerate() {
        let target_col = indicator_col + i;
        if target_col < SCREEN_COLS {
            screen.draw_char_at(border_row, target_col, byte, BORDER_FG, BORDER_BG);
        }
    }
}
