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
use drivers::keyboard;
use core::fmt::Write;
use crate::arch::interrupts;
use crate::arch::power;

/// Execute a simple command from a line of input.
fn execute_command(screen: &mut Screen, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        screen.print_char(b'\n');
        return;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap();

    match cmd {
        "help" => {
            write!(screen, "Commands:\n").unwrap();
            write!(screen, "  help            - show this help\n").unwrap();
            write!(screen, "  echo <text>     - print text\n").unwrap();
            write!(screen, "  cls             - clear screen\n").unwrap();
            write!(screen, "  color <name>    - set color (white, cyan, green)\n").unwrap();
            write!(screen, "  shutdown        - shutdown the system\n").unwrap();
        }
        "echo" => {
            let rest = line[cmd.len()..].trim_start();
            if !rest.is_empty() {
                write!(screen, "{}\n", rest).unwrap();
            } else {
                screen.print_char(b'\n');
            }
        }
        "cls" | "clear" => {
            screen.clear();
        }
        "color" => {
            if let Some(name) = parts.next() {
                if name.eq_ignore_ascii_case("white") {
                    screen.set_color(Color::White);
                } else if name.eq_ignore_ascii_case("cyan") {
                    screen.set_color(Color::LightCyan);
                } else if name.eq_ignore_ascii_case("green") {
                    screen.set_color(Color::LightGreen);
                } else {
                    write!(screen, "Unknown color: {}\n", name).unwrap();
                }
            } else {
                write!(screen, "Usage: color <white|cyan|green>\n").unwrap();
            }
        }
        "shutdown" => {
            write!(screen, "Shutting down...\n").unwrap();
            power::shutdown();
        }
        _ => {
            write!(screen, "Unknown command: {}\n", cmd).unwrap();
        }
    }
}

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
    // Initialize interrupt handling and the keyboard ring buffer.
    interrupts::init();
    interrupts::register_irq_handler(interrupts::IRQ1_VECTOR, |_| {
        keyboard::handle_irq();
    });
    keyboard::init();
    interrupts::enable();

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
    write!(screen, "System initialized.\n").unwrap();

    // write!() Macro testing
    write!(screen, "Hello\n").unwrap();
    write!(screen, "Number: {}\n", 42).unwrap();
    write!(screen, "X={}, Y={}\n", 10, 20).unwrap();
    write!(screen, "Hex: 0x{:x}\n\n", 255).unwrap();  // 0xff

    // A simple REPL loop
    prompt_loop(&mut screen);
}

/// Infinite prompt loop using read_line; echoes entered lines.
fn prompt_loop(screen: &mut Screen) -> ! {
    loop {
        write!(screen, "> ").unwrap();

        let mut buf = [0u8; 128];
        let len = keyboard::read_line(screen, &mut buf);

        if let Ok(line) = core::str::from_utf8(&buf[..len]) {
            execute_command(screen, line);
        } else {
            write!(screen, "(invalid UTF-8)\n").unwrap();
        }
    }
}
