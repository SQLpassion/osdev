//! Panic handler for the kernel
//!
//! Required for `no_std` environments.

use crate::drivers::screen::{Color, Screen};
use core::fmt::Write;
use core::panic::PanicInfo;

/// Panic handler - called when the kernel panics
///
/// In Phase 1, we just print an error message and halt.
/// Later phases will add stack traces and debugging info.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Initialize the screen
    let mut screen = Screen::new();
    screen.clear();

    screen.set_colors(Color::White, Color::Blue);
    write!(screen, "!!! KERNEL PANIC !!!").unwrap();

    if let Some(location) = info.location() {
        writeln!(screen, "Location: {}:{}", location.file(), location.line()).unwrap();
        writeln!(screen).unwrap();
    }

    if let Some(message) = info.message().as_str() {
        writeln!(screen, "Message: {}", message).unwrap();
    }

    // Halt the CPU
    loop {
        unsafe {
            core::arch::asm!("cli"); // Disable interrupts
            core::arch::asm!("hlt"); // Halt
        }
    }
}
