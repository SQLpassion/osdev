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
#[allow(dead_code)]
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
#[derive(Debug, Clone, Copy)]
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
    f(&mut guard)
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

    /// Write a character to the VGA buffer (volatile write)
    fn write_vga(&self, row: usize, col: usize, ch: VgaChar) {
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `vga_ptr(row, col)` computes an in-bounds MMIO cell address.
        // - Volatile write is required for VGA memory-mapped I/O semantics.
        unsafe {
            ptr::write_volatile(self.vga_ptr(row, col), ch);
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
    }

    /// Print a string. The hardware cursor is updated once at the end,
    /// avoiding costly per-character port I/O.
    pub fn print_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put_char(byte);
        }
        self.update_cursor();
    }

    /// Scroll the screen if necessary (matching C Scroll function)
    fn scroll(&mut self) {
        if self.row >= self.num_rows {
            // Move all lines up by one using volatile accesses.
            // The VGA buffer is MMIO, so every read/write must be volatile
            // to prevent the compiler from reordering or eliding them.
            let count = (self.num_rows - 1) * self.num_cols;
            // SAFETY:
            // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
            // - `src` and `dst` point to valid VGA rows and range is in-bounds.
            // - Volatile accesses preserve MMIO ordering semantics.
            unsafe {
                let dst = self.vga_ptr(0, 0);
                let src = self.vga_ptr(1, 0);
                for i in 0..count {
                    let val = ptr::read_volatile(src.add(i));
                    ptr::write_volatile(dst.add(i), val);
                }
            }

            // Clear the last line
            let blank = VgaChar {
                character: b' ',
                attribute: self.attribute(),
            };

            for col in 0..self.num_cols {
                self.write_vga(self.num_rows - 1, col, blank);
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
