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
        // SAFETY:
        // - `boot_info_raw` is non-null, aligned, and within low memory space.
        // - We check the magic header at this address before dereferencing any other fields.
        let magic = unsafe { *(boot_info_raw as *const u64) };
        if magic == 0x4B414F535F424F4F {
            boot_info::BOOT_INFO_PTR.store(boot_info_raw, core::sync::atomic::Ordering::Release);
            // SAFETY:
            // - The magic check succeeded, indicating the pointer points to a valid BootInfo struct.
            let boot_info = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
            kernel_size = boot_info.kernel_size;
            has_boot_info = true;
            debugln!("Unified BootInfo structure detected!");
        }
    }

    debugln!("Kernel size: {} bytes", kernel_size);
    if has_boot_info {
        let boot_info = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
        debugln!("BootInfo memory map len: {}", boot_info.memory_map_len);

        // NOTE: Do NOT touch the linear framebuffer here. On a BIOS/VBE boot it lives at a high
        // physical address that the bootstrap loader's identity map (low 16 MiB) does not cover,
        // and no page-fault/IDT handler exists yet — a write would fault and triple-fault the CPU.
        // The framebuffer is mapped and painted later, once the VMM is up (see `map_framebuffer`).
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

    // Reserve the firmware-owned page-table frames before any significant allocation.
    // `vmm::init` clones the firmware PML4's top-level entries, so those PDPT/PD/PT
    // frames stay live under the kernel; reserve them now so the PMM never hands one
    // out and corrupts the active page tables.
    // SAFETY: the firmware identity map is still active (CR3 not yet switched) and the
    // PMM is initialized, satisfying `reserve_firmware_page_tables`'s contract.
    unsafe {
        vmm::reserve_firmware_page_tables();
    }
    debugln!("Firmware page-table frames reserved");

    // Prepare IDT/PIC so exception handlers are in place before the CR3 switch.
    interrupts::init();
    debugln!("Interrupt subsystem initialized");

    // Initialize the Virtual Memory Manager. It switches CR3 to a kernel PML4 that is a
    // SUPERSET of the firmware page tables (all firmware mappings + a recursive self-map).
    vmm::init(true);
    debugln!("Virtual Memory Manager initialized");

    // Initialize the Heap Manager
    heap::init(true);
    debugln!("Heap Manager initialized");

    // On a graphics-mode boot (BIOS VBE / UEFI GOP) the linear framebuffer lives at a high
    // physical address the bootstrap identity map does not cover. Now that the VMM is active,
    // identity-map the framebuffer's physical range so it is reachable, then paint a one-time
    // gradient to confirm the pipeline. (Deferred to here precisely because the early pre-VMM
    // path has no fault handler and the framebuffer was unmapped.)
    if booted_via_gop(boot_info_raw, has_boot_info) {
        map_framebuffer(boot_info_raw);
        paint_framebuffer_gradient(boot_info_raw);
        debugln!("Framebuffer mapped and gradient painted");
    }

    // Initialize the PCI subsystem (scans the PCI bus)
    drivers::pci::init();
    debugln!("PCI subsystem initialized");

    // Initialize the high-precision time driver
    drivers::time::init();
    debugln!("Time driver initialized");

    // A UEFI/GOP bring-up has no legacy ATA disk, so the disk-dependent path below (ATA PIO,
    // FAT12, loading the user-space shell from disk) cannot run yet — end execution here in a
    // steady BLACK<->WHITE framebuffer heartbeat. A legacy BIOS boot always has an ATA disk
    // (including the BIOS+VBE graphics path), so it falls through to the disk + scheduler + shell
    // path below. `primary_present()` distinguishes the two without a dedicated boot-source flag.
    if booted_via_gop(boot_info_raw, has_boot_info) && !drivers::ata::primary_present() {
        let mut on = false;
        loop {
            fill_screen(boot_info_raw, if on { 0x00FF_FFFF } else { 0x0000_0000 });
            on = !on;
            let mut i = 0u64;
            while i < 40_000_000 {
                i += 1;
                // SAFETY: volatile read of a stack local to defeat loop elimination.
                unsafe { core::ptr::read_volatile(&i) };
            }
        }
    }

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

/// Returns whether the kernel was booted via a unified BootInfo with a GOP
/// framebuffer (the UEFI path), as opposed to the legacy BIOS/VGA-text path.
fn booted_via_gop(boot_info_raw: u64, has_boot_info: bool) -> bool {
    if !has_boot_info {
        return false;
    }
    // SAFETY: validated BootInfo pointer published in KernelMain.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    bi.video_type == boot_info::VideoModeType::GopFramebuffer && bi.fb_info.base_address != 0
}

/// Fills the entire GOP framebuffer with a single `color` (0x00RRGGBB). No-op when
/// not booted via GOP. Used for the end-of-boot heartbeat on the UEFI path.
fn fill_screen(boot_info_raw: u64, color: u32) {
    if !booted_via_gop(boot_info_raw, true) {
        return;
    }
    // SAFETY: validated BootInfo pointer; the framebuffer is mapped (firmware identity,
    // preserved by the kernel PML4 superset).
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    let fb = bi.fb_info;
    let fb_ptr = fb.base_address as *mut u32;
    let mut y = 0u32;
    while y < fb.height {
        let row = (y * fb.pixels_per_scanline) as isize;
        let mut x = 0u32;
        while x < fb.width {
            // SAFETY: bounds-checked; framebuffer mapped + writable.
            unsafe { fb_ptr.offset(row + x as isize).write_volatile(color) };
            x += 1;
        }
        y += 1;
    }
}

/// Identity-maps the linear framebuffer's physical range into the kernel address space.
///
/// On a BIOS/VBE boot the bootstrap loader only identity-maps the low 16 MiB, but the linear
/// framebuffer reported in `BootInfo` lives at a high physical address (typically the
/// 0xC000_0000–0xFFFF_FFFF MMIO window). This walks the framebuffer byte range page by page and
/// maps each 4 KiB page identity (VA == PA, present + writable) via the VMM, so the existing
/// `fb.base_address`-relative writes are valid. No-op when not booted via GOP/VBE.
///
/// Must run after `vmm::init()` (uses the VMM recursive mapping) and `pmm::init()` (intermediate
/// page tables are allocated from the PMM). Pages already mapped (e.g. by UEFI firmware) are
/// skipped, so the call is safe on both the BIOS and UEFI paths.
fn map_framebuffer(boot_info_raw: u64) {
    if !booted_via_gop(boot_info_raw, true) {
        return;
    }
    // SAFETY: validated BootInfo pointer published in KernelMain.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    let fb = bi.fb_info;
    if fb.base_address == 0 || fb.size == 0 {
        return;
    }

    let start = fb.base_address & !0xFFFu64;
    let end = fb.base_address + fb.size as u64;
    let mut addr = start;
    while addr < end {
        // Only create a fresh mapping when the page is genuinely unmapped (the BIOS/VBE case,
        // where the loader maps just the low 16 MiB). On UEFI the firmware already maps the
        // framebuffer — frequently with 2 MiB / 1 GiB huge pages — and that mapping is cloned
        // into the kernel PML4. We MUST detect that and skip it: `is_va_mapped` is huge-page
        // aware, whereas a 4 KiB-only check would miss the huge mapping and make
        // `map_virtual_to_physical` walk into a huge PDE, corrupting the framebuffer/page tables.
        if !vmm::is_va_mapped(addr) {
            vmm::map_virtual_to_physical(addr, addr);
        }
        addr += 0x1000;
    }
    debugln!(
        "Framebuffer identity-mapped: phys 0x{:x}..0x{:x} ({} bytes)",
        start,
        end,
        fb.size
    );
}

/// Paints a one-time RGB gradient across the whole framebuffer to confirm the graphics pipeline.
///
/// No-op when not booted via GOP/VBE. The framebuffer must already be mapped (see
/// [`map_framebuffer`]).
fn paint_framebuffer_gradient(boot_info_raw: u64) {
    if !booted_via_gop(boot_info_raw, true) {
        return;
    }
    // SAFETY: validated BootInfo pointer published in KernelMain.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    let fb = bi.fb_info;
    let fb_ptr = fb.base_address as *mut u32;
    for y in 0..fb.height {
        for x in 0..fb.width {
            let r = (x * 255 / fb.width) & 0xFF;
            let g = (y * 255 / fb.height) & 0xFF;
            let b = 128u32;
            let color = (r << 16) | (g << 8) | b;
            let offset = (y * fb.pixels_per_scanline + x) as isize;
            // SAFETY:
            // - The framebuffer range was identity-mapped by `map_framebuffer`.
            // - `offset` is within bounds (derived from scanline pitch and height).
            unsafe {
                fb_ptr.offset(offset).write_volatile(color);
            }
        }
    }
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
