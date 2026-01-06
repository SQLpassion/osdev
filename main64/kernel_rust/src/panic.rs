//! Panic handler for the kernel
//!
//! Required for `no_std` environments.

use core::panic::PanicInfo;
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
    screen.print_str("\n!!! KERNEL PANIC !!!\n");

    if let Some(location) = info.location()
    {
        screen.print_str("Location: ");
        screen.print_str(location.file());
        screen.print_str(":");
        screen.print_int(location.line() as u32, 10);
        screen.print_str("\n");
    }

    if let Some(message) = info.message().as_str()
    {
        screen.print_str("Message: ");
        screen.print_str(message);
        screen.print_str("\n");
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
