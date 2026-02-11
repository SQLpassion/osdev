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
mod syscall;
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
/// Kernel higher-half base used to translate symbol VAs to physical offsets.
const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;
/// One mapped user page used as initial ring-3 stack backing store.
const USER_SERIAL_TASK_STACK_PAGE_VA: u64 = vmm::USER_STACK_TOP - 0x1000;
/// Initial user RSP used when creating the demo task frame.
const USER_SERIAL_TASK_STACK_TOP: u64 = vmm::USER_STACK_TOP - 16;
/// User-mapped page that stores the serial demo message bytes.
const USER_SERIAL_TASK_MSG_VA: u64 = vmm::USER_CODE_BASE + 0x1000;
const USER_SERIAL_TASK_MSG: &[u8] = b"[ring3] hello from user mode via int 0x80\n";
/// Guards one-time mapping setup for the user serial demo pages.
static mut USER_SERIAL_TASK_PAGES_MAPPED: bool = false;

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
            writeln!(screen, "  userdemo        - run ring-3 serial demo task").unwrap();
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
        "userdemo" => {
            run_user_mode_serial_demo(screen);
        }
        _ => {
            writeln!(screen, "Unknown command: {}", cmd).unwrap();
        }
    }
}

/// Runs a minimal ring-3 smoke test:
/// - map required code/data/stack pages for the demo task,
/// - spawn a user task that performs `WriteSerial` then `Exit`,
/// - block until the task has been removed by the scheduler.
fn run_user_mode_serial_demo(screen: &mut Screen) {
    if let Err(msg) = map_userdemo_task_pages() {
        writeln!(screen, "User demo setup failed: {}", msg).unwrap();
        return;
    }

    if let Err(msg) = write_userdemo_message_page() {
        writeln!(screen, "User demo image failed: {}", msg).unwrap();
        return;
    }

    // TEMPORARY BOOTSTRAP:
    // `userdemo_ring3_task` is compiled as part of kernel text, not loaded from
    // a user binary. Therefore we derive its kernel VA and translate it into the
    // user-code alias window for ring-3 RIP.
    //
    // Once a real program loader exists, RIP must come from the loaded user
    // executable entry point and this translation path should be removed.
    let entry_kernel_va = userdemo_ring3_task as *const () as usize as u64;
    let entry_rip = match kernel_va_to_user_code_va(entry_kernel_va) {
        Some(va) => va,
        None => {
            writeln!(screen, "User demo spawn failed: entry address outside user code alias window").unwrap();
            return;
        }
    };

    // TEMPORARY BOOTSTRAP:
    // We currently run userdemo in the active/shared address space and pass
    // the current CR3 (kernel PML4 root) into `spawn_user_task`.
    // This matches the current mapping path, where `map_userdemo_task_pages`
    // inserts userdemo mappings into that same address space.
    //
    // MUST CHANGE LATER:
    // Switch to per-task/per-process address spaces:
    // - clone kernel PML4 for user (`clone_kernel_pml4_for_user`),
    // - map user code/data/stack into the cloned root,
    // - pass cloned CR3 here,
    // - clean up cloned address space on task exit.
    let cr3 = vmm::get_pml4_address();
    let task_id = match scheduler::spawn_user_task(
        entry_rip,
        USER_SERIAL_TASK_STACK_TOP,
        cr3,
    ) {
        Ok(task_id) => task_id,
        Err(err) => {
            writeln!(screen, "User demo spawn failed: {:?}", err).unwrap();
            return;
        }
    };

    // Wait until the user task is finished, and voluntarily yield the CPU of the REPL task
    while scheduler::task_frame_ptr(task_id).is_some() {
        scheduler::yield_now();
    }

    writeln!(screen, "User demo task finished. Check serial output.").unwrap();
}

/// Ensures every page needed by `userdemo_ring3_task` exists in user space.
///
/// This includes:
/// - code pages for the demo entry and raw syscall helpers (`raw1`/`raw2`),
/// - one user-readable message page,
/// - one writable stack page.
///
/// The setup is idempotent within one kernel boot via `USER_SERIAL_TASK_PAGES_MAPPED`.
///
/// TEMPORARY BOOTSTRAP NOTE:
/// The mapped code pages originate from kernel text and are only aliased into
/// user VA so this demo can execute ring-3 before a full program loader exists.
/// In the final architecture, user code pages are provided by the loader
/// (e.g. ELF segments), not by kernel-text alias mappings.
fn map_userdemo_task_pages() -> Result<(), &'static str> {
    unsafe {
        if USER_SERIAL_TASK_PAGES_MAPPED {
            return Ok(());
        }
    }

    // TEMPORARY BOOTSTRAP:
    // Kernel virtual addresses derived from function pointers (`fn as *const ()`).
    // For each entry we map the containing 4 KiB code page into the user-code
    // alias window so ring-3 can execute the full call chain without fetching
    // instructions from supervisor-only pages.
    //
    // Call chain:
    // userdemo_ring3_task -> raw2(WriteSerial) -> raw1(Exit) -> int 0x80.
    let required_kernel_function_vas: [u64; 5] = [
        userdemo_ring3_task as *const () as usize as u64,
        syscall::user::sys_write_serial as *const () as usize as u64,
        syscall::user::sys_exit as *const () as usize as u64,
        syscall::arch::syscall_raw::raw2 as *const () as usize as u64,
        syscall::arch::syscall_raw::raw1 as *const () as usize as u64,
    ];

    // Reserve two physical 4 KiB frames for userdemo private data pages:
    // - `stack_frame`: backing page for initial ring-3 user stack.
    // - `msg_frame`: backing page that stores USER_SERIAL_TASK_MSG bytes.
    //
    // These are only frame allocations here; actual VA placement and user/rw
    // permissions are applied later via `vmm::map_user_page`.
    let stack_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user serial task stack frame")
    });

    let msg_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user serial task message frame")
    });

    // For every required function address:
    // 1) page-align kernel VA to the containing 4 KiB code page,
    // 2) translate that kernel page VA to physical address,
    // 3) compute the corresponding user-code alias VA,
    // 4) map user alias VA -> same physical code frame (read-only from user side).
    //
    // This gives ring-3 an executable view of the exact kernel text pages needed
    // by the demo syscall call chain, while keeping data pages separate.
    for fn_va in required_kernel_function_vas {
        let code_kernel_page_va = fn_va & !0xFFFu64;
        let code_phys = kernel_va_to_phys(code_kernel_page_va)
            .ok_or("user demo entry has non-higher-half address")?;
        let code_user_page_va = kernel_va_to_user_code_va(code_kernel_page_va)
            .ok_or("user demo function outside user alias window")?;
        vmm::map_user_page(code_user_page_va, code_phys / pmm::PAGE_SIZE, false)
            .map_err(|_| "mapping user code page failed")?;
    }

    vmm::map_user_page(USER_SERIAL_TASK_MSG_VA, msg_frame.pfn, true)
        .map_err(|_| "mapping user message page failed")?;
    vmm::map_user_page(USER_SERIAL_TASK_STACK_PAGE_VA, stack_frame.pfn, true)
        .map_err(|_| "mapping user stack page failed")?;

    unsafe {
        USER_SERIAL_TASK_PAGES_MAPPED = true;
    }
    
    Ok(())
}

/// Writes the static serial demo payload into the user message page.
///
/// The page is pre-zeroed so repeated demo runs always observe deterministic
/// memory content, independent of previous payload lengths.
fn write_userdemo_message_page() -> Result<(), &'static str> {
    unsafe {
        // SAFETY:
        // - `USER_SERIAL_TASK_MSG_VA` is mapped in
        //   `map_userdemo_task_pages`.
        // - Message length fits into one mapped 4 KiB message page.
        core::ptr::write_bytes(USER_SERIAL_TASK_MSG_VA as *mut u8, 0, 0x1000);
        core::ptr::copy_nonoverlapping(
            USER_SERIAL_TASK_MSG.as_ptr(),
            USER_SERIAL_TASK_MSG_VA as *mut u8,
            USER_SERIAL_TASK_MSG.len(),
        );
    }
    Ok(())
}

/// Ring-3 demo entry executed via scheduler `iretq`.
///
/// The body intentionally performs only two direct syscalls:
/// 1. `WriteSerial(msg_ptr, msg_len)`
/// 2. `Exit(0)`
///
/// Keeping the call graph minimal avoids additional user-alias code-page
/// requirements from higher-level wrappers.
extern "C" fn userdemo_ring3_task() -> ! {
    unsafe {
        // SAFETY:
        // - Message VA/len point to a mapped user-readable buffer.
        // - Executes two separate syscalls: WriteSerial, then Exit.
        let _ = syscall::arch::syscall_raw::raw2(
            syscall::SyscallId::WriteSerial as u64,
            USER_SERIAL_TASK_MSG_VA,
            USER_SERIAL_TASK_MSG.len() as u64,
        );

        let _ = syscall::arch::syscall_raw::raw1(syscall::SyscallId::Exit as u64, 0);
        // If the kernel unexpectedly returns from Exit, stay local.
        loop {
            core::hint::spin_loop();
        }
    }
}

#[inline]
/// Converts higher-half kernel VA to physical address by removing base offset.
fn kernel_va_to_phys(kernel_va: u64) -> Option<u64> {
    if kernel_va >= KERNEL_HIGHER_HALF_BASE {
        Some(kernel_va - KERNEL_HIGHER_HALF_BASE)
    } else {
        None
    }
}

#[inline]
/// Maps a kernel symbol VA into the configured user code alias window.
fn kernel_va_to_user_code_va(kernel_va: u64) -> Option<u64> {
    syscall::user_alias_va_for_kernel(
        vmm::USER_CODE_BASE,
        vmm::USER_CODE_SIZE,
        KERNEL_HIGHER_HALF_BASE,
        kernel_va,
    )
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
