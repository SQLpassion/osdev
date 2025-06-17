#![no_std]
#![no_main]

use core::panic::PanicInfo;

// This function is called during panic
#[panic_handler]
fn panic(_info: &PanicInfo) -> !
{
    loop {}
}

// Startup message
static BOOT_MESSAGE: &[u8] = b"This is a free-standing x64 kernel written entirely in Rust!";

// Main entry
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> !
{
    let vga_buffer = 0xb8000 as *mut u8;

    // Clear the screen
    clear_screen();

    unsafe
    {
        for i in 0.. BOOT_MESSAGE.len()
        {
            *vga_buffer.offset(i as isize * 2) = BOOT_MESSAGE[i];
            *vga_buffer.offset(i as isize * 2 + 1) = 0x2;
        }
    }

    loop {}
}

// Clears the screen
fn clear_screen()
{
    let vga_buffer = 0xb8000 as *mut u8;

    unsafe
    {
        for i in 0.. 80 * 25
        {
            *vga_buffer.offset(i as isize * 2) = b' ';
        }
    }
}