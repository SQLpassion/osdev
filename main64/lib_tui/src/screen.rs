//! User-space VGA screen abstraction backed by an 80×25 frame buffer.
//!
//! Widgets write to a local `[u16; 2000]` back-buffer via `with_screen`.
//! Calling `Screen::flush` transfers the buffer to the kernel via the
//! `WriteFramebuffer` syscall, which blits it to VGA MMIO in one step.
//!
//! # Cell encoding
//! Each `u16` encodes one VGA text cell: `(attr << 8) | ascii`.
//! Attribute byte layout: `(bg << 4) | fg` — standard VGA format.

use lib_kaos::console::flush_screen;

/// VGA screen height (rows).
pub const SCREEN_ROWS: usize = 25;
/// VGA screen width (columns).
pub const SCREEN_COLS: usize = 80;

/// Total number of cells in one full frame (80 * 25 = 2000 cells).
const FRAME_SIZE: usize = SCREEN_ROWS * SCREEN_COLS;

/// VGA 4-bit color indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black      = 0,
    Blue       = 1,
    Green      = 2,
    Cyan       = 3,
    Red        = 4,
    Magenta    = 5,
    Brown      = 6,
    LightGray  = 7,
    DarkGray   = 8,
    LightBlue  = 9,
    LightGreen = 10,
    LightCyan  = 11,
    LightRed   = 12,
    Pink       = 13,
    Yellow     = 14,
    White      = 15,
}

/// User-space screen: a 2000-cell back-buffer with a single-syscall flush path.
pub struct Screen {
    /// Flattened row-major grid storing the encoded character and color cell values.
    buffer: [u16; FRAME_SIZE],
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    /// Create a blank screen (zero-initialized buffer, goes into .bss).
    ///
    /// The buffer is filled with actual content on the first `draw_frame` call,
    /// so the initial zero value is never visible to the user.
    pub const fn new() -> Self {
        Self { buffer: [0; FRAME_SIZE] }
    }

    /// Packs raw character and colors into a single 16-bit VGA cell representation.
    #[inline(always)]
    fn make_cell(ch: u8, fg: Color, bg: Color) -> u16 {
        let attr = ((bg as u8) << 4) | (fg as u8);
        ((attr as u16) << 8) | (ch as u16)
    }

    /// Write `text` at absolute (`row`, `col`). Characters beyond the right edge are clipped.
    pub fn draw_at(&mut self, row: usize, col: usize, text: &str, fg: Color, bg: Color) {
        if row >= SCREEN_ROWS { return; }
        let attr = ((bg as u8) << 4) | (fg as u8);
        
        // Step 1: Iterate over bytes and write them to the buffer, zipping with column sequence.
        for (c, byte) in (col..).zip(text.bytes()) {
            if c >= SCREEN_COLS { break; }
            self.buffer[row * SCREEN_COLS + c] = ((attr as u16) << 8) | (byte as u16);
        }
    }

    /// Fill a rectangular region with a single character.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_rect(&mut self, row: usize, col: usize, width: usize, height: usize, ch: u8, fg: Color, bg: Color) {
        let cell = Self::make_cell(ch, fg, bg);
        // Step 1: Iterate and bound rows/columns within screen borders.
        for r in row..(row + height).min(SCREEN_ROWS) {
            for c in col..(col + width).min(SCREEN_COLS) {
                self.buffer[r * SCREEN_COLS + c] = cell;
            }
        }
    }

    /// Write a single character at (`row`, `col`).
    pub fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        if row >= SCREEN_ROWS || col >= SCREEN_COLS { return; }
        self.buffer[row * SCREEN_COLS + col] = Self::make_cell(ch, fg, bg);
    }

    /// Draw a single-line CP437 box border.
    pub fn draw_box(&mut self, row: usize, col: usize, width: usize, height: usize, fg: Color, bg: Color) {
        if width < 2 || height < 2 { return; }
        // CP437 frame characters.
        const TL: u8 = 0xDA; // ┌
        const TR: u8 = 0xBF; // ┐
        const BL: u8 = 0xC0; // └
        const BR: u8 = 0xD9; // ┘
        const H:  u8 = 0xC4; // ─
        const V:  u8 = 0xB3; // │

        let last_col = col + width - 1;
        let last_row = row + height - 1;

        // Draw top corners and line.
        self.draw_char_at(row, col, TL, fg, bg);
        for c in col + 1..last_col { self.draw_char_at(row, c, H, fg, bg); }
        self.draw_char_at(row, last_col, TR, fg, bg);

        // Draw side columns.
        for r in row + 1..last_row {
            self.draw_char_at(r, col, V, fg, bg);
            self.draw_char_at(r, last_col, V, fg, bg);
        }

        // Draw bottom corners and line.
        self.draw_char_at(last_row, col, BL, fg, bg);
        for c in col + 1..last_col { self.draw_char_at(last_row, c, H, fg, bg); }
        self.draw_char_at(last_row, last_col, BR, fg, bg);
    }

    /// Blit the back-buffer to VGA via the `WriteFramebuffer` syscall.
    pub fn flush(&self) {
        let _ = flush_screen(&self.buffer);
    }
}

/// Global singleton screen for single-threaded Ring-3 programs.
static mut SCREEN: Screen = Screen::new();

/// Execute `f` with exclusive mutable access to the global screen back-buffer.
///
/// Does NOT flush to VGA — call `screen.flush()` explicitly when a full frame is ready.
///
/// # Safety (internal invariant)
/// KAOS user programs run cooperatively (single-threaded per address space).
/// There is exactly one task executing at a time, so the `static mut` access
/// has no data races.
pub fn with_screen<R>(f: impl FnOnce(&mut Screen) -> R) -> R {
    // SAFETY: single-threaded cooperative execution — no concurrent access possible.
    unsafe { f(&mut *core::ptr::addr_of_mut!(SCREEN)) }
}
