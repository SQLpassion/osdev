//! Panic handler for the kernel
//!
//! Required for `no_std` environments.

use core::panic::PanicInfo;
use core::fmt::Write;
use crate::drivers::screen::{Screen, Color};

/// Panic handler - called when the kernel panics
///
/// In Phase 1, we just print an error message and halt.
/// Later phases will add stack traces and debugging info.
#[panic_handler]
fn panic(info: &PanicInfo) -> !
{
    // Initialize the screen
    let mut screen = Screen::new();
    screen.clear();
    
    screen.set_colors(Color::White, Color::Blue);
    write!(screen, "\n!!! KERNEL PANIC !!!\n").unwrap();

    if let Some(location) = info.location()
    {
        write!(screen, "Location: {}:{}", location.file(), location.line()).unwrap();
        write!(screen, "\n").unwrap();
    }

    if let Some(message) = info.message().as_str()
    {
        write!(screen, "Message: {}\n", message).unwrap();
    }

    // Halt the CPU
    loop
    {
        unsafe 
        {
            core::arch::asm!("cli");  // Disable interrupts
            core::arch::asm!("hlt");  // Halt
        }
    }
}
