//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]
#![allow(dead_code)]

extern crate alloc;

mod allocator;
mod arch;
mod boot_info;
mod drivers;
mod io;
mod logging;
mod memory;
mod panic;
#[cfg_attr(not(test), allow(dead_code))]
mod process;
mod scheduler;
mod sync;
mod syscall;
mod tui;
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

/// Kernel entry point - called from bootloader (kaosldr_64 or kaosldr_uefi)
///
/// The function signature has been generalized to accept a raw argument:
/// - In legacy modes (and existing tests), it receives `kernel_size`.
/// - In the unified bootloader mode, it receives a pointer to a `BootInfo` structure.
///
/// # Safety
/// This function is called from assembly with the argument in RDI.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(boot_info_raw: u64) -> ! {
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

    // Check if the argument is a valid pointer to a BootInfo structure by matching the magic.
    //
    // WHY WE NEED THIS COMPATIBILITY LAYER:
    // 1. Integration Tests Compatibility: All 20+ integration tests (under `tests/`) define
    //    their own minimal entry points as `KernelMain(_kernel_size: u64)`. When these tests are
    //    booted via the BIOS loader, they expect the parameter to represent the raw size or they
    //    completely ignore the parameter (indicated by the underscore). However, to prevent any
    //    test code from interpreting the `BootInfo` pointer address as a size, or crashing if a
    //    test uses it, we check the magic signature.
    // 2. Bootloader/Kernel Version Mismatches: If a newer kernel is booted by an older loader
    //    that only passes the raw `kernel_size` integer (e.g. 300,000 bytes) in RDI, dereferencing
    //    it blindly as a pointer would cause an immediate Page Fault and a subsequent CPU triple
    //    fault. Checking the magic ensures safe fallback to legacy size handling.
    //
    // SAFETY:
    // - We check if the address is aligned and non-null to avoid invalid dereferencing.
    // - Low physical memory is identity mapped at boot.
    let mut kernel_size = boot_info_raw;
    let mut has_boot_info = false;
    if boot_info_raw > 0x1000 && boot_info_raw.is_multiple_of(8) {
        let magic = unsafe { *(boot_info_raw as *const u64) };
        if magic == 0x4B414F535F424F4F {
            let boot_info = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
            kernel_size = boot_info.kernel_size;
            has_boot_info = true;
            debugln!("Unified BootInfo structure detected!");
        }
    }

    debugln!("Kernel size: {} bytes", kernel_size);
    if has_boot_info {
        debugln!("BootInfo memory map len: {}", unsafe { (*(boot_info_raw as *const boot_info::BootInfo)).memory_map_len });
    }

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

    // Initialize the PCI subsystem (scans the PCI bus)
    drivers::pci::init();
    debugln!("PCI subsystem initialized");

    // Initialize the high-precision time driver
    drivers::time::init();
    debugln!("Time driver initialized");


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

    // Spawn the user-space shell task from the FAT12 disk
    let shell_pid =
        process::exec_from_fat12("shell.bin").expect("failed to spawn SHELL.BIN user-mode task");

    scheduler::start();
    debugln!(
        "Scheduler started with keyboard worker + SHELL.BIN (PID {})",
        shell_pid
    );

    // Enable interrupts — the first timer tick will preempt into a task.
    interrupts::enable();

    // Block until the root shell exits, then shut down cleanly.
    // If the user calls `exit` in the root shell, there is no parent to
    // return to — shutting down is the only sensible response.
    scheduler::wait_for_task_exit(shell_pid as usize);
    arch::power::shutdown()
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
