//! x86 Port I/O operations
//!
//! These functions provide low-level access to x86 I/O ports,
//! mirroring the C functions: inb, outb, inw, outw, inl, outl

use core::arch::asm;

/// Read a byte from the specified I/O port
///
/// # Safety
/// Port I/O is inherently unsafe as it can affect hardware state.
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Write a byte to the specified I/O port
///
/// # Safety
/// Port I/O is inherently unsafe as it can affect hardware state.
#[inline]
unsafe fn outb(port: u16, value: u8) {
    asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Read a word (16-bit) from the specified I/O port
///
/// # Safety
/// Port I/O is inherently unsafe as it can affect hardware state.
#[inline]
unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    asm!(
        "in ax, dx",
        out("ax") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Write a word (16-bit) to the specified I/O port
///
/// # Safety
/// Port I/O is inherently unsafe as it can affect hardware state.
#[inline]
unsafe fn outw(port: u16, value: u16) {
    asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Read a double word (32-bit) from the specified I/O port
///
/// # Safety
/// Port I/O is inherently unsafe as it can affect hardware state.
#[inline]
unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    asm!(
        "in eax, dx",
        out("eax") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Write a double word (32-bit) to the specified I/O port
///
/// # Safety
/// Port I/O is inherently unsafe as it can affect hardware state.
#[inline]
unsafe fn outl(port: u16, value: u32) {
    asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") value,
        options(nomem, nostack, preserves_flags)
    );
}

/// Typed wrapper for a specific I/O port (byte-sized)
#[derive(Debug, Clone, Copy)]
pub struct PortByte {
    port: u16,
}

impl PortByte {
    /// Create a new port wrapper
    pub const fn new(port: u16) -> Self {
        Self { port }
    }

    /// Read from this port
    ///
    /// # Safety
    /// Port I/O is inherently unsafe.
    #[inline]
    pub unsafe fn read(&self) -> u8 {
        inb(self.port)
    }

    /// Write to this port
    ///
    /// # Safety
    /// Port I/O is inherently unsafe.
    #[inline]
    pub unsafe fn write(&self, value: u8) {
        outb(self.port, value)
    }
}

/// Typed wrapper for a specific I/O port (word-sized)
#[derive(Debug, Clone, Copy)]
pub struct PortWord {
    port: u16,
}

impl PortWord {
    /// Create a new port wrapper
    pub const fn new(port: u16) -> Self {
        Self { port }
    }

    /// Read from this port
    ///
    /// # Safety
    /// Port I/O is inherently unsafe.
    #[inline]
    pub unsafe fn read(&self) -> u16 {
        inw(self.port)
    }

    /// Write to this port
    ///
    /// # Safety
    /// Port I/O is inherently unsafe.
    #[inline]
    pub unsafe fn write(&self, value: u16) {
        outw(self.port, value)
    }
}
