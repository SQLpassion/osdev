//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]

extern crate alloc;

mod allocator;
mod arch;
mod drivers;
mod io;
mod logging;
mod memory;
mod panic;
#[cfg_attr(not(test), allow(dead_code))]
mod process;
mod repl;
mod scheduler;
mod sync;
mod syscall;
mod user_tasks;

use crate::arch::fpu;
use crate::arch::gdt;
use crate::arch::interrupts;
use crate::memory::heap;
use crate::memory::pmm;
use crate::memory::vmm;
use drivers::keyboard;
use drivers::serial;

/// Kernel higher-half base used to translate symbol VAs to physical offsets.
const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Zeroes the BSS section using linker-provided boundaries.
///
/// Physical hardware does not guarantee zeroed RAM, so every static variable
/// initialised to zero (spinlocks, atomics, arrays, …) would contain garbage
/// without this step. QEMU happens to zero memory, hiding the problem.
#[inline(always)]
unsafe fn zero_bss() {
    extern "C" {
        static __bss_start: u8;
        static __bss_end: u8;
    }
    let start = &__bss_start as *const u8 as *mut u8;
    let end = &__bss_end as *const u8;
    let len = end as usize - start as usize;
    core::ptr::write_bytes(start, 0, len);
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
pub extern "C" fn KernelMain(kernel_size: u64) -> ! {
    // Zero BSS before touching any static variable — physical hardware
    // does not guarantee zeroed RAM (QEMU does, hiding this bug).
    // SAFETY:
    // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
    // - Called exactly once at early boot before static state is used.
    // - Linker symbols define a valid writable BSS range.
    unsafe {
        zero_bss();
    }

    // Initialize debug serial output first for early debugging
    serial::init();
    debugln!("KAOS Rust Kernel starting...");
    debugln!("Kernel size: {} bytes", kernel_size);

    // Store kernel size for the REPL task banner.
    repl::set_kernel_size(kernel_size);

    // Initialize GDT/TSS so ring-3 transitions have a valid architectural base.
    gdt::init();
    debugln!("GDT/TSS initialized");

    // Initialize the FPU subsystem and capture the default FPU state template.
    // Must run after GDT (needs ring-0 context) and before IDT (the #NM handler
    // installed by interrupts::init() relies on fpu::init() having run).
    fpu::init();
    debugln!("FPU/SSE subsystem initialized");

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

    // Initialize the ATA PIO driver
    drivers::ata::init();
    debugln!("ATA PIO driver initialized");

    // Initialize the FAT12 file system (loads root directory from disk)
    io::fat12::init();
    debugln!("FAT12 file system initialized");

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
    scheduler::spawn_kernel_task(repl::repl_task).expect("failed to spawn REPL task");
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
        // SAFETY:
        // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
        // - `hlt` is valid in ring 0 and used for intentional idle waiting.
        // - Interrupt handlers wake the CPU and resume control flow.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Converts higher-half kernel VA to physical address by removing base offset.
fn kernel_va_to_phys(kernel_va: u64) -> Option<u64> {
    if kernel_va >= KERNEL_HIGHER_HALF_BASE {
        Some(kernel_va - KERNEL_HIGHER_HALF_BASE)
    } else {
        None
    }
}

/// Maps a kernel symbol VA into the configured user code alias window.
fn kernel_va_to_user_code_va(kernel_va: u64) -> Option<u64> {
    syscall::user_alias_va_for_kernel(
        vmm::USER_CODE_BASE,
        vmm::USER_CODE_SIZE,
        KERNEL_HIGHER_HALF_BASE,
        kernel_va,
    )
}
