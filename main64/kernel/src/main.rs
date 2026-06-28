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
mod console;
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

    // Dynamic console initialization based on the boot-time video mode.
    let video_type = if has_boot_info {
        // SAFETY:
        // - `boot_info_raw` was validated above in `KernelMain` to ensure it points to a valid `BootInfo` structure.
        // - Dereferencing is read-only and within bounds.
        let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
        bi.video_type
    } else {
        boot_info::VideoModeType::VgaText
    };
    console::init(video_type);
    debugln!("Kernel console initialized");

    // On a graphics-mode boot (BIOS VBE / UEFI/Linear Framebuffer) the linear framebuffer lives at a high
    // physical address the bootstrap identity map does not cover. Now that the VMM is active,
    // identity-map the framebuffer's physical range so it is reachable, then paint a one-time
    // gradient to confirm the pipeline. (Deferred to here precisely because the early pre-VMM
    // path has no fault handler and the framebuffer was unmapped.)
    if booted_via_framebuffer(boot_info_raw, has_boot_info) {
        map_framebuffer(boot_info_raw);
        debugln!("Framebuffer mapped");

        // SAFETY:
        // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
        let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
        let fb = bi.fb_info;
        crate::console::with_console(|console| {
            // Ensure the screen is fully cleared before the first console output.
            console.clear();
            let _ = writeln!(
                console,
                "VBE Framebuffer active: {}x{} px (stride: {}, base: 0x{:x})",
                fb.width, fb.height, fb.pixels_per_scanline, fb.base_address
            );
        });
    }

    // Initialize the PCI subsystem (scans the PCI bus)
    drivers::pci::init();
    debugln!("PCI subsystem initialized");

    // Initialize the high-precision time driver
    drivers::time::init();
    debugln!("Time driver initialized");

    // A UEFI/Framebuffer bring-up has no legacy ATA disk, so the disk-dependent path below (ATA PIO,
    // FAT12, loading the user-space shell from disk) cannot run yet — end execution here in a
    // steady BLACK<->WHITE framebuffer heartbeat. A legacy BIOS boot always has an ATA disk
    // (including the BIOS+VBE graphics path), so it falls through to the disk + scheduler + shell
    // path below. `primary_present()` distinguishes the two without a dedicated boot-source flag.
    if booted_via_framebuffer(boot_info_raw, has_boot_info) && !drivers::ata::primary_present() {
        // SAFETY:
        // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
        let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
        let fb = bi.fb_info;

        // Step 1: Clear screen and display successful boot messages on the Framebuffer Console.
        crate::console::with_console(|console| {
            console.clear();
            let _ = writeln!(console, "========================================");
            let _ = writeln!(console, "   kaos64 Kernel UEFI Boot Successful   ");
            let _ = writeln!(console, "========================================");
            let _ = writeln!(console, "Linear Framebuffer console is active.");
            let _ = writeln!(
                console,
                "Resolution: {}x{} px (stride: {})",
                fb.width, fb.height, fb.pixels_per_scanline
            );
            let _ = writeln!(
                console,
                "Framebuffer physical address: 0x{:x}",
                fb.base_address
            );
            let _ = writeln!(console, "No legacy ATA disk detected.");
            let _ = writeln!(console, "System halted.");
        });

        // --- START AHCI VERIFICATION (Step 1) ---
        drivers::ahci::init();
        let mut sector = [0u8; 512];
        let read_result = drivers::ahci::read_sectors(&mut sector, 1, 1);
        let efi_part_found = &sector[0..8] == b"EFI PART";

        crate::console::with_console(|console| {
            let _ = writeln!(console, "AHCI read_sectors result: {:?}", read_result);
            let _ = writeln!(
                console,
                "GPT Signature matches 'EFI PART': {}",
                efi_part_found
            );
        });

        debugln!("AHCI First 16 bytes: {:02X?}", &sector[0..16]);
        // --- END AHCI VERIFICATION ---

        // Step 2: Halt the CPU in the low-power idle loop.
        idle_loop();
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

/// Returns whether the kernel was booted via a unified BootInfo with a
/// framebuffer (the graphics path), as opposed to the legacy BIOS/VGA-text path.
fn booted_via_framebuffer(boot_info_raw: u64, has_boot_info: bool) -> bool {
    if !has_boot_info {
        return false;
    }
    // SAFETY:
    // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
    // - This structure is published in `KernelMain` and has been validated by the bootloader.
    // - The memory range is mapped and valid for read access.
    // - Structure alignment is guaranteed by `#[repr(C)]`.
    // - If `boot_info_raw` was null or pointing to invalid memory, this dereference would trigger a page fault.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    bi.video_type == boot_info::VideoModeType::Framebuffer && bi.fb_info.base_address != 0
}

/// Fills the entire framebuffer with a single `color` (0x00RRGGBB). No-op when
/// not booted via a framebuffer. Used for the end-of-boot heartbeat on the UEFI path.
fn fill_screen(boot_info_raw: u64, color: u32) {
    if !booted_via_framebuffer(boot_info_raw, true) {
        return;
    }
    // SAFETY:
    // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
    // - The structure is mapped, valid for reads, and alignment is guaranteed by `#[repr(C)]`.
    // - If it was invalid, the dereference would cause a page fault.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    let fb = bi.fb_info;
    let fb_ptr = fb.base_address as *mut u32;
    let mut y = 0u32;
    while y < fb.height {
        let row = (y * fb.pixels_per_scanline) as isize;
        let mut x = 0u32;
        while x < fb.width {
            // SAFETY:
            // - The framebuffer is mapped and writable.
            // - `row + x` is within the valid physical bounds of the framebuffer as provided by `fb.size`.
            // - Aliasing and concurrency are prevented because we are in a single-threaded early boot phase or panic state.
            // - If the address was unmapped or misconfigured, it would trigger a page fault.
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
/// `fb.base_address`-relative writes are valid. No-op when not booted via a framebuffer.
///
/// Must run after `vmm::init()` (uses the VMM recursive mapping) and `pmm::init()` (intermediate
/// page tables are allocated from the PMM). Pages already mapped (e.g. by UEFI firmware) are
/// skipped, so the call is safe on both the BIOS and UEFI paths.
fn map_framebuffer(boot_info_raw: u64) {
    if !booted_via_framebuffer(boot_info_raw, true) {
        return;
    }
    // SAFETY:
    // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
    // - The structure is mapped, valid for reads, and alignment is guaranteed by `#[repr(C)]`.
    // - If it was invalid, the dereference would cause a page fault.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };
    let fb = bi.fb_info;
    if fb.base_address == 0 || fb.size == 0 {
        return;
    }

    // Configure PAT MSR (0x277) to set PAT1 (bits 8..15) to Write-Combining (0x01).
    // The default value is usually 0x0007_0406_0007_0406 (PAT1 = 0x04 = WT).
    // SAFETY: Writing to the PAT MSR is safe on x86_64, as we are in Ring 0.
    unsafe {
        let mut pat = crate::arch::msr::rdmsr(0x277);
        pat &= !(0xFF << 8); // Clear PAT1
        pat |= 0x01 << 8; // Set PAT1 to Write-Combining (WC)
        crate::arch::msr::wrmsr(0x277, pat);
    }

    let start = fb.base_address & !0xFFFu64;
    let end = fb.base_address + fb.size as u64;
    let mut addr = start;
    while addr < end {
        // Only create a fresh mapping when the page is genuinely unmapped (the BIOS/VBE case,
        // where the loader maps just the low 16 MiB). On UEFI the firmware already maps the
        // framebuffer.
        if !vmm::is_va_mapped(addr) {
            vmm::map_virtual_to_physical_wc(addr, addr);
        }
        addr += 0x1000;
    }

    // Pass 2: For any mappings that already existed (e.g. UEFI firmware mappings),
    // update their page table flags to activate Write-Combining via PAT1 (PWT=1).
    // This safely modifies both 4 KiB and huge pages.
    vmm::configure_wc_mapping(fb.base_address, fb.size as u64);

    // SAFETY: Flush CPU caches to ensure PAT memory type changes are visible and no
    // stale lines with incorrect caching types (like WT or WB) remain in the cache.
    // The Intel SDM requires this after PAT modification.
    unsafe { crate::arch::cache::wbinvd() };

    debugln!(
        "Framebuffer identity-mapped: phys 0x{:x}..0x{:x} ({} bytes)",
        start,
        end,
        fb.size
    );
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
