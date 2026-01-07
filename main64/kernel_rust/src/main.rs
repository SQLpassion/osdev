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
use core::fmt::Write;

/// Kernel entry point - called from bootloader (kaosldr_64)
///
/// The function signature matches the C version:
/// `void KernelMain(int KernelSize)`
///
/// # Safety
/// This function is called from assembly with the kernel size in RDI.
#[no_mangle]
#[link_section = ".text.boot"]
#[allow(unconditional_panic)]
pub extern "C" fn KernelMain(kernel_size: i32) -> !
{
    // Initialize the screen
    let mut screen = Screen::new();
    screen.clear();

    // Print welcome message
    screen.set_color(Color::LightGreen);
    write!(screen, "========================================\n").unwrap();
    write!(screen, "    KAOS - Klaus' Operating System\n").unwrap();
    write!(screen, "         Rust Kernel v0.1.0\n").unwrap();
    write!(screen, "========================================\n\n").unwrap();

    screen.set_color(Color::White);
    write!(screen, "Kernel loaded successfully!\n").unwrap();
    write!(screen, "Kernel size: {} bytes\n\n", kernel_size).unwrap();

    // write!() Macro testing
    write!(screen, "Hello\n").unwrap();
    write!(screen, "Number: {}\n", 42).unwrap();
    write!(screen, "X={}, Y={}\n", 10, 20).unwrap();
    write!(screen, "Hex: 0x{:x}\n\n", 255).unwrap();  // 0xff

    screen.set_color(Color::LightCyan);
    write!(screen, "System initialized. Halting CPU.\n").unwrap();

    // Halt the CPU
    loop
    {
        unsafe
        {
            core::arch::asm!("hlt");
        }
    }
}
