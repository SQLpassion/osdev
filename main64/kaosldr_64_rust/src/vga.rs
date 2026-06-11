use core::arch::asm;
use core::fmt;

// Constants matching misc.h
pub const VIDEO_MEMORY: *mut u8 = 0xB8000 as *mut u8;
pub const ROWS: usize = 25;
pub const COLS: usize = 80;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum VgaColor {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGrey = 7,
    DarkGrey = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    LightMagenta = 13,
    LightBrown = 14,
    White = 15,
}

/// A writer structure to handle printing via VGA text mode.
pub struct VgaWriter {
    row: usize,
    col: usize,
    attribute: u8,
}

impl VgaWriter {
    /// Create a new VgaWriter with default white-on-black attribute.
    pub const fn new() -> Self {
        VgaWriter {
            row: 0,
            col: 0,
            attribute: VgaColor::White as u8,
        }
    }

    /// Clear the screen and reset the cursor.
    pub fn clear_screen(&mut self) {
        // SAFETY:
        // - VIDEO_MEMORY is mapped and valid for ROWS * COLS * 2 bytes of text buffer.
        unsafe {
            // Loop through each character cell in the VGA text buffer (2 bytes per cell: ASCII + attribute).
            // We use a simple while loop here to avoid the overhead, complex trait calls,
            // and panic check machinery of the standard StepBy iterator in debug builds.
            let mut offset = 0;
            while offset < ROWS * COLS * 2 {
                *VIDEO_MEMORY.add(offset) = b' ';
                *VIDEO_MEMORY.add(offset + 1) = self.attribute;
                offset += 2;
            }
        }
        self.row = 0;
        self.col = 0;
        self.move_cursor();
    }

    /// Move the hardware screen cursor to the current row and column.
    pub fn move_cursor(&self) {
        let cursor_location = (self.row * COLS + self.col) as u16;

        // SAFETY:
        // - Direct CPU port access for VGA CRT controller.
        unsafe {
            outb(0x3D4, 14);
            outb(0x3D5, (cursor_location >> 8) as u8);
            outb(0x3D4, 15);
            outb(0x3D5, cursor_location as u8);
        }
    }

    /// Write a single byte to the VGA text frame buffer.
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.row += 1;
                self.col = 0;
            }
            b'\t' => {
                self.col = (self.col + 8) & !(8 - 1);
            }
            _ => {
                if self.row >= ROWS {
                    // Simple wrapping or scroll if we exceed bounds.
                    // For the loader, we just wrap around to the top or clip.
                    self.row = 0;
                }
                if self.col >= COLS {
                    self.row += 1;
                    self.col = 0;
                }

                let offset = (self.row * COLS * 2) + (self.col * 2);
                // SAFETY:
                // - Offset is within the bounds of the 80x25 VGA buffer.
                unsafe {
                    *VIDEO_MEMORY.add(offset) = byte;
                    *VIDEO_MEMORY.add(offset + 1) = self.attribute;
                }
                self.col += 1;
            }
        }
        self.move_cursor();
    }
}

impl fmt::Write for VgaWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

/// Reads a single byte from the specified port.
///
/// # Safety
/// The caller must ensure that the port is valid and does not cause side effects.
pub unsafe fn inb(port: u16) -> u8 {
    let ret: u8;
    // SAFETY:
    // - CPU instruction `in` executes with the specified port register.
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") ret, options(nomem, nostack, preserves_flags));
    }
    ret
}

/// Reads a single 16-bit word from the specific port.
///
/// # Safety
/// The caller must ensure that the port is valid and does not cause side effects.
pub unsafe fn inw(port: u16) -> u16 {
    let ret: u16;
    // SAFETY:
    // - CPU instruction `in` executes with the specified port register.
    unsafe {
        asm!("in ax, dx", in("dx") port, out("ax") ret, options(nomem, nostack, preserves_flags));
    }
    ret
}

/// Writes a single byte to the specified port.
///
/// # Safety
/// The caller must ensure that the port is valid and does not cause side effects.
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY:
    // - CPU instruction `out` executes with the specified port register.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Writes a single 16-bit word to the specified port.
///
/// # Safety
/// The caller must ensure that the port is valid and does not cause side effects.
#[allow(dead_code)]
pub unsafe fn outw(port: u16, value: u16) {
    // SAFETY:
    // - CPU instruction `out` executes with the specified port register.
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

/// Writes a single 32-bit dword to the specified port.
///
/// # Safety
/// The caller must ensure that the port is valid and does not cause side effects.
#[allow(dead_code)]
pub unsafe fn outl(port: u16, value: u32) {
    // SAFETY:
    // - CPU instruction `out` executes with the specified port register.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}
