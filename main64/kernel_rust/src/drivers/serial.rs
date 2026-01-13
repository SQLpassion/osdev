//! Serial Port Driver for Debug Output
//!
//! Implements a simple serial port driver for COM1 (0x3F8) that can be used
//! for debug output. When running under QEMU, use `-serial file:debug.log`
//! to redirect the output to a file on the host system.

use crate::arch::port::PortByte;
use core::fmt;

/// Standard COM1 base port address
const COM1_PORT: u16 = 0x3F8;

/// Serial port register offsets
const DATA_REGISTER: u16 = 0;           // Read/Write data
const INTERRUPT_ENABLE: u16 = 1;        // Interrupt enable register
const FIFO_CONTROL: u16 = 2;            // FIFO control register
const LINE_CONTROL: u16 = 3;            // Line control register
const MODEM_CONTROL: u16 = 4;           // Modem control register
const LINE_STATUS: u16 = 5;             // Line status register

/// Line status register bits
const LINE_STATUS_THRE: u8 = 0x20;      // Transmitter holding register empty

/// Serial port driver for debug output
pub struct Serial {
    base_port: u16,
}

impl Serial {
    /// Create a new serial port driver for COM1
    pub const fn new() -> Self {
        Self {
            base_port: COM1_PORT,
        }
    }

    /// Initialize the serial port
    ///
    /// Sets up 115200 baud, 8 data bits, no parity, 1 stop bit (8N1)
    pub fn init(&self) {
        unsafe {
            let interrupt_enable = PortByte::new(self.base_port + INTERRUPT_ENABLE);
            let fifo_control = PortByte::new(self.base_port + FIFO_CONTROL);
            let line_control = PortByte::new(self.base_port + LINE_CONTROL);
            let modem_control = PortByte::new(self.base_port + MODEM_CONTROL);

            // Disable all interrupts
            interrupt_enable.write(0x00);

            // Enable DLAB (Divisor Latch Access Bit) to set baud rate
            line_control.write(0x80);

            // Set divisor to 1 (115200 baud)
            // Divisor = 115200 / baud_rate
            // For 115200 baud: divisor = 1
            let divisor_low = PortByte::new(self.base_port + DATA_REGISTER);
            let divisor_high = PortByte::new(self.base_port + INTERRUPT_ENABLE);
            divisor_low.write(0x01);    // Low byte of divisor
            divisor_high.write(0x00);   // High byte of divisor

            // Configure line: 8 bits, no parity, 1 stop bit (8N1)
            // Also clears DLAB
            line_control.write(0x03);

            // Enable FIFO, clear them, with 14-byte threshold
            fifo_control.write(0xC7);

            // Enable IRQs, set RTS/DSR
            modem_control.write(0x0B);
        }
    }

    /// Check if the transmit buffer is empty and ready for data
    fn is_transmit_empty(&self) -> bool {
        unsafe {
            let line_status = PortByte::new(self.base_port + LINE_STATUS);
            (line_status.read() & LINE_STATUS_THRE) != 0
        }
    }

    /// Write a single byte to the serial port
    pub fn write_byte(&self, byte: u8) {
        // Wait for transmit buffer to be empty
        while !self.is_transmit_empty() {
            core::hint::spin_loop();
        }

        unsafe {
            let data = PortByte::new(self.base_port + DATA_REGISTER);
            data.write(byte);
        }
    }

    /// Write a string to the serial port
    pub fn write_str(&self, s: &str) {
        for byte in s.bytes() {
            // Convert LF to CRLF for proper line endings in log files
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }
}

/// Implement fmt::Write for Serial so we can use write!() and writeln!()
impl fmt::Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        Serial::write_str(self, s);
        Ok(())
    }
}

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global serial port instance for debug output
struct DebugSerial {
    serial: UnsafeCell<Serial>,
    initialized: AtomicBool,
}

// Safety: Serial port access is inherently single-threaded in our kernel
// (no SMP support), and we use atomic flag for initialization state.
unsafe impl Sync for DebugSerial {}

static DEBUG_SERIAL: DebugSerial = DebugSerial {
    serial: UnsafeCell::new(Serial::new()),
    initialized: AtomicBool::new(false),
};

/// Initialize the debug serial port
///
/// Call this early in kernel initialization to enable debug output.
pub fn init() {
    unsafe {
        (*DEBUG_SERIAL.serial.get()).init();
    }
    DEBUG_SERIAL.initialized.store(true, Ordering::Release);
}

/// Write formatted debug output to the serial port
///
/// This function is used by the debug! macro.
#[doc(hidden)]
pub fn _debug_print(args: fmt::Arguments) {
    use fmt::Write;
    if DEBUG_SERIAL.initialized.load(Ordering::Acquire) {
        unsafe {
            let _ = (*DEBUG_SERIAL.serial.get()).write_fmt(args);
        }
    }
}

/// Debug output macro - works like print! but outputs to serial port
///
/// Usage:
/// ```
/// debug!("Hello, world!");
/// debug!("Value: {}", 42);
/// debug!("Multiple values: {} and {}", x, y);
/// ```
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::drivers::serial::_debug_print(format_args!($($arg)*))
    };
}

/// Debug output macro with newline - works like println! but outputs to serial port
///
/// Usage:
/// ```
/// debugln!("Hello, world!");
/// debugln!("Value: {}", 42);
/// debugln!();  // Just a newline
/// ```
#[macro_export]
macro_rules! debugln {
    () => {
        $crate::debug!("\n")
    };
    ($($arg:tt)*) => {
        $crate::debug!("{}\n", format_args!($($arg)*))
    };
}
