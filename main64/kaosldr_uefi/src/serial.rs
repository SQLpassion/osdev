//! Minimal 16550 UART (COM1) driver for early loader output.
//!
//! UEFI provides `ConOut` for on-screen text, but on a headless host — a build container or
//! real hardware without a monitor — the serial port is the reliable output channel. It is the
//! same one the KAOS test runner already consumes via QEMU's `-serial stdio`, and the debug
//! channel intended for real hardware. Writing here uses raw port I/O and needs no firmware
//! services, so it keeps working after `ExitBootServices()` as well.

use core::arch::asm;

/// Base I/O port of the first serial controller (COM1).
const COM1: u16 = 0x3F8;

/// Writes a byte to an x86 I/O port.
///
/// # Safety
/// The caller must ensure `port` is a valid I/O port for an 8-bit write with no unintended
/// side effects.
unsafe fn outb(port: u16, value: u8) {
    asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

/// Reads a byte from an x86 I/O port.
///
/// # Safety
/// The caller must ensure `port` is a valid I/O port for an 8-bit read.
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

/// Initialises COM1 to 38400 baud, 8 data bits, no parity, one stop bit, FIFO enabled.
pub fn init() {
    // SAFETY: the standard, well-known 16550 UART initialisation sequence on the fixed COM1 ports.
    unsafe {
        outb(COM1 + 1, 0x00); // Disable all UART interrupts.
        outb(COM1 + 3, 0x80); // Enable DLAB to program the baud-rate divisor.
        outb(COM1, 0x03);     // Divisor low byte  (115200 / 3 = 38400 baud).
        outb(COM1 + 1, 0x00); // Divisor high byte.
        outb(COM1 + 3, 0x03); // 8N1; also clears DLAB.
        outb(COM1 + 2, 0xC7); // Enable and clear the FIFO, 14-byte trigger threshold.
        outb(COM1 + 4, 0x0B); // Assert RTS/DTR and enable OUT2.
    }
}

/// Writes a single byte to COM1, busy-waiting until the transmit holding register is empty.
pub fn write_byte(byte: u8) {
    // SAFETY: reads the line-status register and writes the data register of the fixed COM1 port.
    unsafe {
        while inb(COM1 + 5) & 0x20 == 0 {} // Wait for the THR-empty bit (LSR bit 5).
        outb(COM1, byte);
    }
}

/// Writes a string to COM1 byte by byte (the caller is responsible for any `\r\n` handling).
pub fn write_str(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}
