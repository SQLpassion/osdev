//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]

mod apps;
mod arch;
mod drivers;
mod panic;

use crate::arch::interrupts;
use crate::arch::power;
use core::fmt::Write;
use drivers::keyboard;
use drivers::screen::{Color, Screen};

const PAGE_SIZE: u64 = 4096;

const BIB_OFFSET: usize = 0x1000;
const MEMORYMAP_OFFSET: usize = 0x1200;

#[repr(C)]
struct BiosInformationBlock {
    year: i32,
    month: i16,
    day: i16,
    hour: i16,
    minute: i16,
    second: i16,
    memory_map_entries: i16,
    max_memory: i64,
    available_page_frames: i64,
    physical_memory_layout: *mut PhysicalMemoryLayout,
}

#[repr(C)]
struct BiosMemoryRegion {
    start: u64,
    size: u64,
    region_type: u32,
}

#[repr(C)]
struct PhysicalMemoryLayout {
    memory_region_count: u32,
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
pub extern "C" fn KernelMain(kernel_size: i32) -> ! {
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
            print_memory_map(screen);
        }
        _ => {
            writeln!(screen, "Unknown command: {}", cmd).unwrap();
        }
    }
}

/// Prints the memory map that we have obtained from the BIOS in x16 Real Mode.
fn print_memory_map(screen: &mut Screen) {
    // Mutable view so we can mirror the C code: compute MaxMemory and AvailablePageFrames here.
    let bib = unsafe {
        &mut *(BIB_OFFSET as *mut BiosInformationBlock)
    };
    
    let region = MEMORYMAP_OFFSET as *const BiosMemoryRegion;
    
    let entry_count = bib.memory_map_entries as usize;

    // Reset and recompute MaxMemory / AvailablePageFrames like the C physical memory manager.
    bib.max_memory = 0;
    bib.available_page_frames = 0;

    // Print header
    writeln!(screen, "{} Memory Map entries found.", entry_count).unwrap();
    
    // Loop over each entry
    for i in 0..entry_count {
        let current_region = unsafe {
            &*region.add(i)
        };
        
        // Set color based on region type
        if current_region.region_type == 1 {
            // Available
            screen.set_color(Color::LightGreen);

            // Track totals for available regions (matches C code behavior)
            bib.max_memory = bib.max_memory.wrapping_add(current_region.size as i64);
            bib.available_page_frames = bib
                .available_page_frames
                .wrapping_add((current_region.size / PAGE_SIZE) as i64);
        } else {
            // Everything else
            screen.set_color(Color::LightRed);
        }
        
        // Start address
        write!(screen, "0x{:010x}", current_region.start).unwrap();
        
        // End address (use wrapping arithmetic to avoid overflow)
        let end_addr = current_region.start.wrapping_add(current_region.size).wrapping_sub(1);
        write!(screen, " - 0x{:010x}", end_addr).unwrap();
        
        // Size in hex
        write!(screen, " Size: 0x{:09x}", current_region.size).unwrap();
        
        // Size in KB
        write!(screen, " {} KB", current_region.size / 1024).unwrap();
        
        // Size in MB if applicable
        if current_region.size > 1024 * 1024 {
            write!(screen, " = {} MB", current_region.size / 1024 / 1024).unwrap();
        }
        
        // Memory region type
        let region_type_str = match current_region.region_type {
            1 => "Available",
            2 => "Reserved",
            3 => "ACPI Reclaim",
            4 => "ACPI NVS Memory",
            _ => "Unknown",
        };
        writeln!(screen, " ({})", region_type_str).unwrap();
    }
    
    // Reset color to white
    screen.set_color(Color::White);
    
    // Max memory
    writeln!(screen, "Max Memory: {} MB", bib.max_memory / 1024 / 1024 + 1).unwrap();
}