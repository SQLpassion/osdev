//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]

extern crate alloc;

mod apps;
mod arch;
mod allocator;
mod drivers;
mod logging;
mod memory;
mod panic;
mod scheduler;
mod sync;

use crate::arch::interrupts;
use crate::arch::power;
use crate::arch::gdt;
use crate::memory::bios;
use crate::memory::heap;
use crate::memory::pmm;
use crate::memory::vmm;
use core::fmt::Write;
use drivers::keyboard;
use drivers::screen::{Color, Screen};
use drivers::serial;

/// Kernel size stored by `KernelMain` so that the REPL task can display it
/// in the welcome banner.  Written once before the scheduler starts, read
/// only afterwards — no synchronization needed.
static mut KERNEL_SIZE: u64 = 0;

const PATTERN_DELAY_SPINS: usize = 500_000;
const VGA_TEXT_COLS: usize = 80;

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

    // Store kernel size for the REPL task banner.
    // SAFETY: Written once before any task is spawned; read-only afterwards.
    unsafe { KERNEL_SIZE = kernel_size; }

    // Initialize GDT/TSS so ring-3 transitions have a valid architectural base.
    gdt::init();
    debugln!("GDT/TSS initialized");

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
    heap::init(true);
    debugln!("Heap Manager initialized");

    // Initialize interrupt handling and the keyboard ring buffer.
    interrupts::register_irq_handler(interrupts::IRQ1_KEYBOARD_VECTOR, |_, frame| {
        keyboard::handle_irq();
        frame as *mut _
    });

    interrupts::init_periodic_timer(250);

    keyboard::init();
    debugln!("Keyboard initialized");

    // Initialize the scheduler and spawn the system tasks.
    // Interrupts stay disabled until the scheduler is fully set up so the
    // first timer tick sees a consistent state.
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());
    scheduler::spawn_kernel_task(keyboard::keyboard_worker_task)
        .expect("failed to spawn keyboard worker task");
    scheduler::spawn_kernel_task(repl_task)
        .expect("failed to spawn REPL task");
    scheduler::start();
    debugln!("Scheduler started with keyboard worker + REPL task");

    // Enable interrupts — the first timer tick will preempt into a task.
    interrupts::enable();

    // Idle loop: the CPU halts until each timer interrupt.  The scheduler
    // selects a ready task on every tick; when all tasks are blocked the
    // CPU stays here in low-power halt.
    idle_loop()
}

/// Low-power idle loop entered after the scheduler is started.
fn idle_loop() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// REPL task entry point — runs as a scheduled kernel task.
///
/// Creates its own `Screen` instance (VGA MMIO wrapper) and enters the
/// interactive command prompt loop.
extern "C" fn repl_task() -> ! {
    let mut screen = Screen::new();
    screen.clear();

    // Print welcome message
    let kernel_size = unsafe { KERNEL_SIZE };
    screen.set_color(Color::LightGreen);
    writeln!(screen, "========================================").unwrap();
    writeln!(screen, "    KAOS - Klaus' Operating System").unwrap();
    writeln!(screen, "         Rust Kernel v0.1.0").unwrap();
    writeln!(screen, "========================================").unwrap();
    screen.set_color(Color::White);
    writeln!(screen, "Kernel loaded successfully!").unwrap();
    writeln!(screen, "Kernel size: {} bytes\n", kernel_size).unwrap();

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
            writeln!(screen, "  mtdemo          - run VGA multitasking demo (press q to stop)").unwrap();
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
                let snapshot = screen.save();
                match apps::spawn_app(app_name) {
                    Ok(task_id) => {
                        while scheduler::task_frame_ptr(task_id).is_some() {
                            scheduler::yield_now();
                        }
                        screen.restore(&snapshot);
                    }
                    Err(apps::RunAppError::UnknownApp) => {
                        writeln!(screen, "Unknown app: {}", app_name).unwrap();
                        writeln!(screen, "Use 'apps' to list available applications.").unwrap();
                    }
                    Err(apps::RunAppError::SpawnFailed(err)) => {
                        writeln!(screen, "Failed to launch app task: {:?}", err).unwrap();
                    }
                }
            } else {
                writeln!(screen, "Usage: run <appname>").unwrap();
                writeln!(screen, "Use 'apps' to list available applications.").unwrap();
            }
        }
        "mtdemo" => {
            run_multitasking_vga_demo(screen);
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

fn run_multitasking_vga_demo(screen: &mut Screen) {
    let task_ids = spawn_pattern_tasks();

    writeln!(screen, "Multitasking demo active (rows 22-24). Press q to stop.").unwrap();
    loop {
        let ch = keyboard::read_char_blocking();
        if ch == b'q' || ch == b'Q' {
            terminate_pattern_tasks(&task_ids);
            while !pattern_tasks_terminated(&task_ids) {
                scheduler::yield_now();
            }
            writeln!(screen, "\nMultitasking demo stopped.").unwrap();
            return;
        }
    }
}

fn spawn_pattern_tasks() -> [usize; 3] {
    [
        scheduler::spawn_kernel_task(vga_pattern_task_a).expect("failed to spawn VGA pattern task A"),
        scheduler::spawn_kernel_task(vga_pattern_task_b).expect("failed to spawn VGA pattern task B"),
        scheduler::spawn_kernel_task(vga_pattern_task_c).expect("failed to spawn VGA pattern task C"),
    ]
}

fn pattern_tasks_terminated(task_ids: &[usize; 3]) -> bool {
    task_ids
        .iter()
        .all(|task_id| scheduler::task_frame_ptr(*task_id).is_none())
}

fn terminate_pattern_tasks(task_ids: &[usize; 3]) {
    for task_id in task_ids {
        let _ = scheduler::terminate_task(*task_id);
    }
}

fn draw_progress_bar(screen: &mut Screen, row: usize, color: Color, label: u8, progress: usize) {
    let fill = progress.min(VGA_TEXT_COLS);
    screen.set_color(color);
    screen.set_cursor(row, 0);
    for idx in 0..VGA_TEXT_COLS {
        let ch = if idx < fill { b'#' } else { b'.' };
        if idx == 0 {
            screen.print_char(label);
        } else {
            screen.print_char(ch);
        }
    }
}

/// Generic VGA progress-bar task used to visualize task switching on live screen output.
fn vga_pattern_task(row: usize, label: u8, color: Color, step: usize) -> ! {
    let mut screen = Screen::new();
    let mut progress = 0usize;

    loop {
        draw_progress_bar(&mut screen, row, color, label, progress);
        progress = (progress + step) % (VGA_TEXT_COLS + 1);

        for _ in 0..PATTERN_DELAY_SPINS {
            core::hint::spin_loop();
        }

        scheduler::yield_now();
    }
}

extern "C" fn vga_pattern_task_a() -> ! {
    vga_pattern_task(22, b'A', Color::LightCyan, 1)
}

extern "C" fn vga_pattern_task_b() -> ! {
    vga_pattern_task(23, b'B', Color::Yellow, 2)
}

extern "C" fn vga_pattern_task_c() -> ! {
    vga_pattern_task(24, b'C', Color::Pink, 3)
}
