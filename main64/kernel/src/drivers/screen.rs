//! VGA Text Mode Driver
//!
//! Implements the same functionality as the C `screen.c` driver.
//! Video memory is at physical address 0xB8000, which is mapped to
//! virtual address 0xFFFF8000000B8000 in the higher-half kernel.

use crate::arch::port::PortByte;
use crate::sync::spinlock::SpinLock;
use core::fmt;
use core::ptr;

/// VGA text buffer base address (higher-half kernel mapping)
const VGA_BUFFER: usize = 0xFFFF8000000B8000;

/// VGA control ports for cursor
const VGA_CTRL_REGISTER: u16 = 0x3D4;
const VGA_DATA_REGISTER: u16 = 0x3D5;

/// Default screen dimensions
const DEFAULT_COLS: usize = 80;
const DEFAULT_ROWS: usize = 25;

/// VGA Colors (matching C defines in screen.h)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// Represents a VGA character cell (character + attribute byte)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct VgaChar {
    character: u8,
    attribute: u8,
}

/// Screen driver state (mirrors C ScreenLocation struct)
pub struct Screen {
    row: usize,
    col: usize,
    foreground: Color,
    background: Color,
    num_cols: usize,
    num_rows: usize,
    back_buffer: [VgaChar; DEFAULT_ROWS * DEFAULT_COLS],
    front_buffer: [VgaChar; DEFAULT_ROWS * DEFAULT_COLS],
    suspend_flush: bool,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

/// Lock-free VGA writer used exclusively by panic paths.
///
/// This writer deliberately bypasses `GLOBAL_SCREEN` and therefore cannot
/// deadlock on `SpinLock<Screen>` if a panic occurs while the lock is held.
pub struct PanicScreenWriter {
    row: usize,
    col: usize,
    attribute: u8,
}

impl PanicScreenWriter {
    /// Creates a panic writer with fixed colors.
    pub const fn new(foreground: Color, background: Color) -> Self {
        Self {
            row: 0,
            col: 0,
            attribute: ((background as u8) << 4) | (foreground as u8),
        }
    }

    /// Clears the full VGA text buffer without using any locks.
    pub fn clear(&mut self) {
        // Step 1: blank every visible VGA cell with the configured attribute.
        for row in 0..DEFAULT_ROWS {
            for col in 0..DEFAULT_COLS {
                let offset = row * DEFAULT_COLS + col;
                let cell_ptr = (VGA_BUFFER + offset * 2) as *mut VgaChar;
                let blank = VgaChar {
                    character: b' ',
                    attribute: self.attribute,
                };
                // SAFETY:
                // - This requires `unsafe` because raw pointer MMIO writes are outside Rust's static checks.
                // - `cell_ptr` points to one in-bounds VGA text cell.
                // - Volatile write is required for deterministic MMIO behavior.
                unsafe {
                    ptr::write_volatile(cell_ptr, blank);
                }
            }
        }

        // Step 2: reset cursor state in this lock-free writer.
        self.row = 0;
        self.col = 0;
    }

    #[inline]
    fn put_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.row = (self.row + 1).min(DEFAULT_ROWS - 1);
                self.col = 0;
            }
            _ => {
                if self.row >= DEFAULT_ROWS {
                    self.row = DEFAULT_ROWS - 1;
                }
                if self.col >= DEFAULT_COLS {
                    self.col = 0;
                    self.row = (self.row + 1).min(DEFAULT_ROWS - 1);
                }
                let offset = self.row * DEFAULT_COLS + self.col;
                let cell_ptr = (VGA_BUFFER + offset * 2) as *mut VgaChar;
                let cell = VgaChar {
                    character: byte,
                    attribute: self.attribute,
                };
                // SAFETY:
                // - This requires `unsafe` because raw pointer MMIO writes are outside Rust's static checks.
                // - `cell_ptr` is computed from bounded row/col coordinates.
                // - Volatile write is required for deterministic MMIO behavior.
                unsafe {
                    ptr::write_volatile(cell_ptr, cell);
                }
                self.col += 1;
            }
        }
    }
}

impl fmt::Write for PanicScreenWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.put_byte(byte);
        }
        Ok(())
    }
}

/// Wrapper around a global screen instance.
///
/// Mirrors the `with_pmm` access pattern: all mutable access is routed through
/// a closure, avoiding `static mut` references in call sites. Uses a SpinLock
/// for thread-safe access with interrupt disabling.
struct GlobalScreen {
    inner: SpinLock<Screen>,
}

impl GlobalScreen {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(Screen::new()),
        }
    }
}

/// Global VGA writer shared across kernel paths.
static GLOBAL_SCREEN: GlobalScreen = GlobalScreen::new();

/// Executes `f` with mutable access to the shared global screen writer.
///
/// This is the screen-side equivalent of `with_pmm`: callers provide a closure
/// and do not handle global state directly. This function is thread-safe: it
/// acquires a spinlock that disables interrupts while the closure executes,
/// preventing preemption.
pub fn with_screen<R>(f: impl FnOnce(&mut Screen) -> R) -> R {
    let mut guard = GLOBAL_SCREEN.inner.lock();
    let res = f(&mut guard);
    guard.flush();
    res
}

impl Screen {
    /// Initialize the screen driver
    pub const fn new() -> Self {
        Self {
            row: 0,
            col: 0,
            foreground: Color::White,
            background: Color::Black,
            num_cols: DEFAULT_COLS,
            num_rows: DEFAULT_ROWS,
            back_buffer: [VgaChar { character: b' ', attribute: 0x07 }; DEFAULT_ROWS * DEFAULT_COLS],
            front_buffer: [VgaChar { character: 0, attribute: 0 }; DEFAULT_ROWS * DEFAULT_COLS],
            suspend_flush: false,
        }
    }

    /// Calculate the attribute byte from foreground and background colors
    fn attribute(&self) -> u8 {
        ((self.background as u8) << 4) | (self.foreground as u8)
    }

    /// Get pointer to VGA buffer at specific row/col
    fn vga_ptr(&self, row: usize, col: usize) -> *mut VgaChar {
        let offset = row * self.num_cols + col;
        (VGA_BUFFER + offset * 2) as *mut VgaChar
    }

    /// Write a character to the back-buffer (no MMIO, safe memory write)
    fn write_vga(&mut self, row: usize, col: usize, ch: VgaChar) {
        if row < self.num_rows && col < self.num_cols {
            let offset = row * self.num_cols + col;
            self.back_buffer[offset] = ch;
        }
    }

    /// Set whether screen flushing to physical MMIO is suspended.
    #[allow(dead_code)]
    pub fn set_suspend_flush(&mut self, suspend: bool) {
        self.suspend_flush = suspend;
    }

    /// Flush the back-buffer to the physical VGA screen.
    /// Only cells that differ from the cached frontbuffer are written to MMIO.
    pub fn flush(&mut self) {
        if self.suspend_flush {
            return;
        }
        for i in 0..(self.num_rows * self.num_cols) {
            let back_cell = self.back_buffer[i];
            let front_cell = self.front_buffer[i];

            if back_cell.character != front_cell.character || back_cell.attribute != front_cell.attribute {
                let row = i / self.num_cols;
                let col = i % self.num_cols;
                let vga_ptr = self.vga_ptr(row, col);

                // SAFETY:
                // - `row` and `col` are in bounds (since `i < 2000`).
                // - `vga_ptr` points to the memory-mapped physical VGA buffer.
                // - Volatile write is required for MMIO behavior.
                unsafe {
                    ptr::write_volatile(vga_ptr, back_cell);
                }
                
                self.front_buffer[i] = back_cell;
            }
        }
    }

    /// Overwrite the back-buffer with a flat 80×25 frame transmitted from user space.
    ///
    /// Each element of `cells` encodes one VGA text cell as `(attr << 8) | ascii`,
    /// matching the `WriteFramebuffer` syscall ABI.
    pub fn blit_framebuffer(&mut self, cells: &[u16]) {
        let count = cells.len().min(self.num_rows * self.num_cols);
        for (i, &cell) in cells[..count].iter().enumerate() {
            self.back_buffer[i] = VgaChar {
                character: cell as u8,
                attribute: (cell >> 8) as u8,
            };
        }
    }

    /// Set the current text color
    pub fn set_color(&mut self, color: Color) {
        self.foreground = color;
    }

    /// Clear the screen
    pub fn clear(&mut self) {
        let blank = VgaChar {
            character: b' ',
            attribute: self.attribute(),
        };

        for row in 0..self.num_rows {
            for col in 0..self.num_cols {
                self.write_vga(row, col, blank);
            }
        }

        self.row = 0;
        self.col = 0;
        self.update_cursor();
        self.flush();
    }

    /// Write a character to the VGA buffer and handle scrolling,
    /// but do NOT update the hardware cursor.
    /// Used internally for batch operations where the cursor
    /// only needs to be updated once after all characters are written.
    fn put_char(&mut self, c: u8) {
        match c {
            b'\n' => {
                self.row += 1;
                self.col = 0;
                self.scroll();
            }
            b'\r' => {
                self.col = 0;
            }
            b'\t' => {
                // Tab to next 8-character boundary
                self.col = (self.col + 8) & !(8 - 1);
                if self.col >= self.num_cols {
                    self.col = 0;
                    self.row += 1;
                }
                self.scroll();
            }
            0x08 => {
                // Backspace
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.num_cols - 1;
                } else {
                    return;
                }

                let blank = VgaChar {
                    character: b' ',
                    attribute: self.attribute(),
                };
                self.write_vga(self.row, self.col, blank);
            }
            _ => {
                if self.col >= self.num_cols {
                    self.row += 1;
                    self.col = 0;
                }

                // Ensure the write target is always in-bounds before touching VGA MMIO.
                // Without this, a wrap at the last screen cell can transiently produce
                // `row == num_rows` and write one row past the visible text buffer.
                if self.row >= self.num_rows {
                    self.scroll();
                }

                let vga_char = VgaChar {
                    character: c,
                    attribute: self.attribute(),
                };

                self.write_vga(self.row, self.col, vga_char);
                self.col += 1;
            }
        }
    }

    /// Print a single character and update the hardware cursor.
    pub fn print_char(&mut self, c: u8) {
        self.put_char(c);
        self.update_cursor();
        self.flush();
    }

    /// Print a string. The hardware cursor is updated once at the end,
    /// avoiding costly per-character port I/O.
    pub fn print_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put_char(byte);
        }
        self.update_cursor();
        self.flush();
    }

    /// Scroll the screen if necessary (matching C Scroll function)
    fn scroll(&mut self) {
        if self.row >= self.num_rows {
            // Step 1: Shift all lines up by 1 in the back-buffer.
            let count = (self.num_rows - 1) * self.num_cols;
            for i in 0..count {
                self.back_buffer[i] = self.back_buffer[i + self.num_cols];
            }

            // Step 2: Clear the last line.
            let blank = VgaChar {
                character: b' ',
                attribute: self.attribute(),
            };

            for col in 0..self.num_cols {
                let offset = (self.num_rows - 1) * self.num_cols + col;
                self.back_buffer[offset] = blank;
            }

            self.row = self.num_rows - 1;
        }
    }

    /// Update the hardware cursor position (matching C MoveCursor function)
    fn update_cursor(&self) {
        let pos = (self.row * self.num_cols + self.col) as u16;

        // SAFETY:
        // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
        // - VGA controller cursor ports are valid on x86 text mode hardware.
        // - Writes only program cursor position registers.
        unsafe {
            let ctrl = PortByte::new(VGA_CTRL_REGISTER);
            let data = PortByte::new(VGA_DATA_REGISTER);

            // High byte
            ctrl.write(14);
            data.write((pos >> 8) as u8);

            // Low byte
            ctrl.write(15);
            data.write(pos as u8);
        }
    }

    /// Set cursor position (0-based)
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.row = row.min(self.num_rows - 1);
        self.col = col.min(self.num_cols - 1);
        self.update_cursor();
    }

    /// Get cursor position (0-based)
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Write `text` directly at (`row`, `col`) with explicit colors, without
    /// advancing the scroll cursor.  Characters that would exceed the right
    /// edge of the screen are silently clipped.
    ///
    /// This is the fundamental TUI primitive: it writes into the VGA buffer
    /// at an absolute position and never triggers scrolling.
    pub fn draw_at(&mut self, row: usize, col: usize, text: &str, fg: Color, bg: Color) {
        if row >= self.num_rows {
            return;
        }

        // Build the attribute byte from the caller-supplied colors.
        let attr = ((bg as u8) << 4) | (fg as u8);

        // Step 1: Iterate over string bytes and write them to VGA memory.
        // We zip the bytes with an infinite sequence starting at `col` to keep
        // track of the column index, stopping if we exceed the screen width.
        for (c, byte) in (col..).zip(text.bytes()) {
            if c >= self.num_cols {
                break;
            }

            let cell = VgaChar {
                character: byte,
                attribute: attr,
            };

            self.write_vga(row, c, cell);
        }
    }

    /// Fill a rectangular region with a single character and explicit colors.
    ///
    /// Used by the TUI engine to blank widget backgrounds before rendering
    /// content, ensuring no stale text from previous frames bleeds through.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_rect(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        ch: u8,
        fg: Color,
        bg: Color,
    ) {
        let attr = ((bg as u8) << 4) | (fg as u8);

        // Iterate row-by-row and column-by-column, clamping to screen bounds.
        for r in row..row.saturating_add(height).min(self.num_rows) {
            for c in col..col.saturating_add(width).min(self.num_cols) {
                let cell = VgaChar {
                    character: ch,
                    attribute: attr,
                };
                self.write_vga(r, c, cell);
            }
        }
    }

    /// Draw a single-line box (border only) using VGA-compatible box-drawing
    /// characters (code page 437).
    ///
    /// The box occupies rows `row..row+height` and columns `col..col+width`.
    /// Interior cells are left untouched; call `fill_rect` first if a blank
    /// background is needed.
    pub fn draw_box(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        fg: Color,
        bg: Color,
    ) {
        if width < 2 || height < 2 {
            return;
        }

        // CP437 single-line box-drawing bytes
        const TL: u8 = 0xDA; // ┌
        const TR: u8 = 0xBF; // ┐
        const BL: u8 = 0xC0; // └
        const BR: u8 = 0xD9; // ┘
        const H: u8 = 0xC4; // ─
        const V: u8 = 0xB3; // │

        let last_col = col + width - 1;
        let last_row = row + height - 1;

        // Step 1: top edge
        self.draw_char_at(row, col, TL, fg, bg);
        for c in col + 1..last_col {
            self.draw_char_at(row, c, H, fg, bg);
        }
        self.draw_char_at(row, last_col, TR, fg, bg);

        // Step 2: side edges (skip corners already drawn)
        for r in row + 1..last_row {
            self.draw_char_at(r, col, V, fg, bg);
            self.draw_char_at(r, last_col, V, fg, bg);
        }

        // Step 3: bottom edge
        self.draw_char_at(last_row, col, BL, fg, bg);
        for c in col + 1..last_col {
            self.draw_char_at(last_row, c, H, fg, bg);
        }
        self.draw_char_at(last_row, last_col, BR, fg, bg);
    }

    /// Write a single byte directly at (`row`, `col`) with explicit colors.
    ///
    /// Internal helper shared by `draw_at` and `draw_box`.  Out-of-bounds
    /// coordinates are silently ignored.
    pub fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        if row >= self.num_rows || col >= self.num_cols {
            return;
        }

        let attr = ((bg as u8) << 4) | (fg as u8);
        let cell = VgaChar {
            character: ch,
            attribute: attr,
        };

        self.write_vga(row, col, cell);
    }

    /// Disable the hardware blinking text cursor completely.
    ///
    /// On real VGA hardware `hide_cursor` (which moves the cursor off-screen)
    /// may not be sufficient because the cursor can still be visible at
    /// position 0 on some implementations.  Setting bit 5 of the Cursor Start
    /// Register (CRTC index 0x0A) disables the cursor entirely per the VGA
    /// specification.
    pub fn disable_hw_cursor(&self) {
        // SAFETY:
        // - Hardware port I/O is inherently outside Rust's memory-safety guarantees.
        // - CRTC register 0x0A (Cursor Start) is a standard VGA register.
        // - Bit 5 = 1 disables the cursor; this is a pure configuration write.
        unsafe {
            let ctrl = crate::arch::port::PortByte::new(VGA_CTRL_REGISTER);
            let data = crate::arch::port::PortByte::new(VGA_DATA_REGISTER);

            // Select Cursor Start Register and set bit 5 (cursor disable).
            ctrl.write(0x0A);
            data.write(0x20);
        }
    }

    /// Re-enable the hardware text cursor after a `disable_hw_cursor` call.
    ///
    /// Programs the cursor as a two-scanline underline at the bottom of the
    /// character cell (scanlines 14–15), which is the conventional appearance
    /// for an 80×25 VGA text mode cursor.
    pub fn enable_hw_cursor(&self) {
        // SAFETY:
        // - Hardware port I/O is inherently outside Rust's memory-safety guarantees.
        // - CRTC registers 0x0A (Cursor Start) and 0x0B (Cursor End) are
        //   standard VGA registers for cursor shape configuration.
        unsafe {
            let ctrl = crate::arch::port::PortByte::new(VGA_CTRL_REGISTER);
            let data = crate::arch::port::PortByte::new(VGA_DATA_REGISTER);

            // Cursor Start: scanline 14, bit 5 clear (cursor enabled).
            ctrl.write(0x0A);
            data.write(0x0E);

            // Cursor End: scanline 15.
            ctrl.write(0x0B);
            data.write(0x0F);
        }
    }

    /// Keep `hide_cursor` for backwards compatibility — delegates to
    /// `disable_hw_cursor` which is more reliable on real hardware.
    #[allow(dead_code)]
    pub fn hide_cursor(&self) {
        self.disable_hw_cursor();
    }

    /// Disable VGA blink mode by clearing bit 3 of the Attribute Controller
    /// Mode Control Register (AC index 0x10).
    ///
    /// # Why this is necessary
    ///
    /// In the default VGA text mode, bit 7 of a character's attribute byte is
    /// the *blink enable* flag.  Because background colors are stored in
    /// attribute bits 6:4 (only 3 bits), colors with value ≥ 8 — such as
    /// `DarkGray (8)`, `LightBlue (9)`, `Pink (13)`, `Yellow (14)` — set bit 7
    /// when shifted into the background position and therefore cause the
    /// character cell to blink on real VGA hardware.
    ///
    /// Clearing bit 3 of AC register 0x10 switches the hardware into
    /// *background intensity* mode: bit 7 of the attribute byte becomes the
    /// high bit of a 4-bit background color field instead of the blink flag.
    /// All 16 colors can then be used as backgrounds without any blinking.
    ///
    /// # Access protocol
    ///
    /// The VGA Attribute Controller uses a shared address/data port (0x3C0)
    /// gated by a flip-flop that must be reset by reading the Input Status
    /// Register 1 (port 0x3DA) before each transaction.
    pub fn disable_blink_mode(&self) {
        // SAFETY:
        // - Hardware port I/O is inherently outside Rust's memory-safety guarantees.
        // - The VGA Attribute Controller ports (0x3C0, 0x3C1, 0x3DA) are
        //   standard x86 VGA registers valid in ring 0.
        // - The access sequence (ISR1 read → index write → data read/write)
        //   follows the VGA specification for AC register access.
        // - Clearing AC[0x10] bit 3 only changes the blink/intensity mode;
        //   it does not affect video timing or other registers.
        unsafe {
            let isr1 = crate::arch::port::PortByte::new(0x3DA); // Input Status Reg 1
            let ac_addr = crate::arch::port::PortByte::new(0x3C0); // AC address / data
            let ac_data_r = crate::arch::port::PortByte::new(0x3C1); // AC data (read-only)

            // Step 1: Reset the AC flip-flop to "address" mode by reading ISR1.
            let _ = isr1.read();

            // Step 2: Select AC register 0x10 with palette-enable bit (0x20) set
            //         so that the screen remains visible during the transaction.
            ac_addr.write(0x10 | 0x20);

            // Step 3: Read the current value of register 0x10.
            let val = ac_data_r.read();

            // Step 4: Reset flip-flop and write back with blink bit (bit 3) cleared.
            let _ = isr1.read();
            ac_addr.write(0x10 | 0x20); // re-select register
            ac_addr.write(val & !0x08); // write with blink bit cleared

            // Step 5: Restore palette-enable so video output is not suppressed.
            ac_addr.write(0x20);
        }
    }

    /// Re-enable VGA blink mode (restore the default VGA text mode behavior).
    ///
    /// After this call, bit 7 of each attribute byte is again interpreted as a
    /// blink flag, and background colors are limited to 0–7.  Call this when
    /// transitioning back from the TUI to a context that needs only 8 bg colors.
    pub fn enable_blink_mode(&self) {
        // SAFETY: same invariants as `disable_blink_mode`.
        unsafe {
            let isr1 = crate::arch::port::PortByte::new(0x3DA);
            let ac_addr = crate::arch::port::PortByte::new(0x3C0);
            let ac_data_r = crate::arch::port::PortByte::new(0x3C1);

            let _ = isr1.read();
            ac_addr.write(0x10 | 0x20);
            let val = ac_data_r.read();
            let _ = isr1.read();
            ac_addr.write(0x10 | 0x20);
            ac_addr.write(val | 0x08); // set blink bit
            ac_addr.write(0x20);
        }
    }
}

// Implement the core::fmt::Write trait so write!() works on Screen
impl fmt::Write for Screen {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.print_str(s);
        Ok(())
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        if c.is_ascii() {
            self.print_char(c as u8);
        } else {
            self.print_char(b'?');
        }
        Ok(())
    }
}
