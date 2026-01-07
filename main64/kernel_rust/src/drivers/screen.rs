//! VGA Text Mode Driver
//!
//! Implements the same functionality as the C `screen.c` driver.
//! Video memory is at physical address 0xB8000, which is mapped to
//! virtual address 0xFFFF8000000B8000 in the higher-half kernel.

use core::ptr;
use crate::arch::port::PortByte;
use core::fmt;

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

impl Screen {
    /// Initialize the screen driver
    pub fn new() -> Self {
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
        unsafe {
            ptr::write_volatile(self.vga_ptr(row, col), ch);
        }
    }

    /// Read a character from the VGA buffer (volatile read)
    fn read_vga(&self, row: usize, col: usize) -> VgaChar {
        unsafe {
            ptr::read_volatile(self.vga_ptr(row, col))
        }
    }

    /// Set the current text color
    pub fn set_color(&mut self, color: Color) {
        self.foreground = color;
    }

    /// Set both foreground and background colors
    pub fn set_colors(&mut self, foreground: Color, background: Color) {
        self.foreground = foreground;
        self.background = background;
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

    /// Print a single character
    pub fn print_char(&mut self, c: u8) {
        match c {
            b'\n' => {
                self.row += 1;
                self.col = 0;
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

                {
                    let blank = VgaChar {
                        character: b' ',
                        attribute: self.attribute(),
                    };
                    self.write_vga(self.row, self.col, blank);
                }
            }
            _ => {
                if self.col >= self.num_cols {
                    self.row += 1;
                    self.col = 0;
                }

                let vga_char = VgaChar {
                    character: c,
                    attribute: self.attribute(),
                };

                self.write_vga(self.row, self.col, vga_char);
                self.col += 1;
            }
        }

        self.scroll();
        self.update_cursor();
    }

    /// Print a string
    pub fn print_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.print_char(byte);
        }
    }

    /// Scroll the screen if necessary (matching C Scroll function)
    fn scroll(&mut self) {
        if self.row >= self.num_rows {
            // Move all lines up by one
            let count = (self.num_rows - 1) * self.num_cols;
            unsafe {
                let dst = self.vga_ptr(0, 0);
                let src = self.vga_ptr(1, 0);
                ptr::copy(src, dst, count);
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
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.row = row.min(self.num_rows - 1);
        self.col = col.min(self.num_cols - 1);
        self.update_cursor();
    }

    /// Get cursor position (0-based)
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
        match c {
            '\n' => self.print_char(b'\n'),
            '\r' => self.print_char(b'\r'),
            '\t' => self.print_char(b'\t'),
            ch => {
                if ch.is_ascii() {
                    self.print_char(ch as u8);
                } else {
                    // Fallback for non-ASCII
                    self.print_char(b'?');
                }
            }
        }
        Ok(())
    }
}
