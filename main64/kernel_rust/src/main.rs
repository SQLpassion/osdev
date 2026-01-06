//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]

mod panic;
mod arch;
mod drivers;

use drivers::screen::{Color, Screen};

/// Kernel entry point - called from bootloader (kaosldr_64)
///
/// The function signature matches the C version:
/// `void KernelMain(int KernelSize)`
///
/// # Safety
/// This function is called from assembly with the kernel size in RDI.
#[no_mangle]
#[allow(unconditional_panic)]
pub extern "C" fn KernelMain(kernel_size: i32) -> !
{
    // Initialize the screen
    let mut screen = Screen::new();
    screen.clear();

    // Print welcome message
    screen.set_color(Color::LightGreen);
    screen.print_str("========================================\n");
    screen.print_str("    KAOS - Klaus' Operating System\n");
    screen.print_str("         Rust Kernel v0.1.0\n");
    screen.print_str("========================================\n\n");
    screen.print_str("Wow, this really works!\n\n");

    screen.set_color(Color::White);
    screen.print_str("Kernel loaded successfully!\n");
    screen.print_str("Kernel size: ");
    screen.print_int(kernel_size as u32, 10);
    screen.print_str(" bytes\n\n");

    screen.set_color(Color::LightCyan);
    screen.print_str("System initialized. Halting CPU.\n");        

    // Halt the CPU
    loop
    {
        unsafe
        {
            core::arch::asm!("hlt");
        }
    }
}
