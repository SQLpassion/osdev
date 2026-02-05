//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]

mod apps;
mod arch;
mod drivers;
mod logging;
mod memory;
mod panic;

use crate::arch::interrupts;
use crate::arch::power;
use crate::memory::bios;
use crate::memory::heap;
use crate::memory::pmm;
use crate::memory::vmm;
use core::fmt::Write;
use drivers::keyboard;
use drivers::screen::{Color, Screen};
use drivers::serial;

/// Kernel entry point - called from bootloader (kaosldr_64)
///
/// The function signature matches the C version:
/// `void KernelMain(int KernelSize)`
///
/// # Safety
/// This function is called from assembly with the kernel size in RDI.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(kernel_size: u64) -> ! {
    // Initialize debug serial output first for early debugging
    serial::init();
    debugln!("KAOS Rust Kernel starting...");
    debugln!("Kernel size: {} bytes", kernel_size);

    // Initialize the Physical Memory Manager
    pmm::init(true);
    debugln!("Physical Memory Manager initialized");

    // Prepare IDT/PIC first so exception handlers are in place before CR3 switch.
    interrupts::init();
    debugln!("Interrupt subsystem initialized");

    // Initialize the Virtual Memory Manager
    vmm::init(true);
    debugln!("Virtual Memory Manager initialized");

    // Initialize the Heap Manager
    heap::init();
    debugln!("Heap Manager initialized");

    // Initialize interrupt handling and the keyboard ring buffer.
    interrupts::register_irq_handler(interrupts::IRQ1_VECTOR, |_| {
        keyboard::handle_irq();
    });
    
    keyboard::init();
    interrupts::enable();
    debugln!("Interrupts enabled");

    // Initialize the screen
    let mut screen = Screen::new();
    screen.clear();

    // Print welcome message
    screen.set_color(Color::LightGreen);
    writeln!(screen, "========================================").unwrap();
    writeln!(screen, "    KAOS - Klaus' Operating System").unwrap();
    writeln!(screen, "         Rust Kernel v0.1.0").unwrap();
    writeln!(screen, "========================================").unwrap();
    screen.set_color(Color::White);
    writeln!(screen, "Kernel loaded successfully!").unwrap();
    writeln!(screen, "Kernel size: {} bytes\n", kernel_size).unwrap();

    // Execute the command prompt loop
    command_prompt_loop(&mut screen);
}

/// Infinite prompt loop using read_line; echoes entered lines.
fn command_prompt_loop(screen: &mut Screen) -> ! {
    loop {
        write!(screen, "> ").unwrap();

        let mut buf = [0u8; 128];
        let len = keyboard::read_line(screen, &mut buf);

        if let Ok(line) = core::str::from_utf8(&buf[..len]) {
            execute_command(screen, line);
        } else {
            writeln!(screen, "(invalid UTF-8)").unwrap();
        }
    }
}

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
            writeln!(screen, "Commands:\n").unwrap();
            writeln!(screen, "  help            - show this help").unwrap();
            writeln!(screen, "  echo <text>     - print text").unwrap();
            writeln!(screen, "  cls             - clear screen").unwrap();
            writeln!(screen, "  color <name>    - set color (white, cyan, green)").unwrap();
            writeln!(screen, "  apps            - list available applications").unwrap();
            writeln!(screen, "  run <app>       - run an application").unwrap();
            writeln!(screen, "  meminfo         - display BIOS memory map").unwrap();
            writeln!(screen, "  pmm [n]         - run PMM self-test (default n=2048)").unwrap();
            writeln!(screen, "  vmmtest [--debug] - run VMM smoke test").unwrap();
            writeln!(screen, "  heaptest        - run heap self-test").unwrap();
            writeln!(screen, "  shutdown        - shutdown the system").unwrap();
        }
        "echo" => {
            let rest = line[cmd.len()..].trim_start();
            if !rest.is_empty() {
                writeln!(screen, "{}", rest).unwrap();
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
                    writeln!(screen, "Unknown color: {}", name).unwrap();
                }
            } else {
                writeln!(screen, "Usage: color <white|cyan|green>").unwrap();
            }
        }
        "shutdown" => {
            writeln!(screen, "Shutting down...").unwrap();
            power::shutdown();
        }
        "apps" => {
            apps::list_apps(screen);
        }
        "run" => {
            if let Some(app_name) = parts.next() {
                if !apps::run_app(app_name, screen) {
                    writeln!(screen, "Unknown app: {}", app_name).unwrap();
                    writeln!(screen, "Use 'apps' to list available applications.").unwrap();
                }
            } else {
                writeln!(screen, "Usage: run <appname>").unwrap();
                writeln!(screen, "Use 'apps' to list available applications.").unwrap();
            }
        }
        "meminfo" => {
            bios::BiosInformationBlock::print_memory_map(screen);
        }
        "pmm" => {
            match (parts.next(), parts.next()) {
                (None, None) => pmm::run_self_test(screen, 2048),
                (Some(n_str), None) => match n_str.parse::<u32>() {
                    Ok(n) if n > 0 => pmm::run_self_test(screen, n),
                    _ => writeln!(screen, "Usage: pmm [n]  (n must be > 0)").unwrap(),
                },
                _ => {
                    writeln!(screen, "Usage: pmm [n]").unwrap();
                }
            }
        }
        "testvmm" | "vmmtest" => {
            let console_debug = match (parts.next(), parts.next()) {
                (None, None) => false,
                (Some("--debug"), None) => true,
                _ => {
                    writeln!(screen, "Usage: vmmtest [--debug]").unwrap();
                    return;
                }
            };

            vmm::set_console_debug_output(console_debug);
            let ok = vmm::test_vmm();
            if console_debug {
                vmm::print_console_debug_output(screen);
            }
            vmm::set_console_debug_output(false);
            if ok {
                writeln!(screen, "VMM test complete (readback OK).").unwrap();
            } else {
                writeln!(screen, "VMM test complete (readback FAILED).").unwrap();
            }
        }
        "heaptest" => {
            heap::run_self_test(screen);
        }
        _ => {
            writeln!(screen, "Unknown command: {}", cmd).unwrap();
        }
    }
}
