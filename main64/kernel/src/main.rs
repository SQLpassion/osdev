//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]
// NOTE (issue #62 / L13): this bin target (`KernelMain`'s crate) declares the
// same module tree as `lib.rs` via private `mod` declarations rather than
// depending on the `kaos_kernel` library crate, so every module is compiled
// twice: once for the library (used by integration tests under `kernel/tests/`)
// and once for this binary. Many `pub fn`/`pub` items exist solely for the
// library side (test-only introspection hooks, self-test entry points, trait
// methods required by `BlockDevice`/`KernelConsole`, etc.) and are never
// called from `KernelMain`'s own control flow, which makes them look dead
// specifically in *this* compilation unit. A crate-wide allow is kept
// (instead of scoping `#[allow(dead_code)]` item-by-item across dozens of
// unrelated files) because that is where the dead code genuinely lives; the
// two functions that were truly unused *anywhere* in the codebase
// (`kernel_va_to_phys`, `kernel_va_to_user_code_va`) have been removed rather
// than hidden behind this allow (verified via a repo-wide grep with zero
// remaining callers).
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
            let boot_info = boot_info::BootInfo::get().unwrap();
            kernel_size = boot_info.kernel_size;
            has_boot_info = true;
            debugln!("Unified BootInfo structure detected!");
        }
    }

    debugln!("Kernel size: {} bytes", kernel_size);
    if has_boot_info {
        let boot_info = boot_info::BootInfo::get().unwrap();
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

    // Enable EFER.NXE so the No-Execute (bit 63) flag the kernel sets on user
    // stack/heap pages is honored. The legacy loader enables this, but the UEFI
    // loader does not — without it, real hardware raises a reserved-bit page
    // fault on the first access to an NX page. Enabling it in the kernel makes
    // this independent of the boot loader.
    arch::msr::enable_no_execute();
    debugln!("EFER.NXE enabled (No-Execute paging active)");

    // Initialize the Physical Memory Manager
    pmm::init(true);
    debugln!("Physical Memory Manager initialized");

    // Reserve the firmware-owned page-table frames before any significant allocation -
    // but only on the firmware-clone fallback path (a boot with no `BootInfo`, e.g. a
    // unit-test kernel). `vmm::init` clones the firmware PML4's top-level entries there,
    // so those PDPT/PD/PT frames stay live under the kernel; reserve them now so the PMM
    // never hands one out and corrupts the active page tables. On the standard
    // kernel-owned-table path (a published `BootInfo`, i.e. every real boot) those
    // sub-tables are never referenced by the new table at all, so reserving them would
    // just leak memory forever instead of letting the PMM reclaim it after the switch.
    let boot_info_published =
        boot_info::BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Acquire) != 0;
    if !boot_info_published {
        // SAFETY: the firmware identity map is still active (CR3 not yet switched) and
        // the PMM is initialized, satisfying `reserve_firmware_page_tables`'s contract.
        unsafe {
            vmm::reserve_firmware_page_tables();
        }
        debugln!("Firmware page-table frames reserved (firmware-clone fallback)");
    }

    // Prepare IDT/PIC so exception handlers are in place before the CR3 switch.
    interrupts::init();
    debugln!("Interrupt subsystem initialized");

    // Initialize the Virtual Memory Manager. On every real boot (a published `BootInfo`)
    // it switches CR3 to a genuinely kernel-owned page-table hierarchy built from the
    // boot memory map; a BootInfo-less boot falls back to a superset clone of the
    // firmware page tables (all firmware mappings + a recursive self-map).
    vmm::init(true);
    debugln!("Virtual Memory Manager initialized");

    // Initialize the Heap Manager
    heap::init(true);
    debugln!("Heap Manager initialized");

    // Dynamic console initialization based on the boot-time video mode.
    let video_type = if has_boot_info {
        let bi = boot_info::BootInfo::get().unwrap();
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

        let bi = boot_info::BootInfo::get().unwrap();
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

    // Both boot paths converge on a single scheduler bring-up that runs the
    // user-space shell. They differ only in how the shell image is obtained:
    //
    // - A UEFI/Framebuffer boot has no legacy ATA disk. The shell lives on the
    //   FAT32 EFI System Partition and is reached through the AHCI controller:
    //   `ahci::init` -> `gpt::find_esp_start_lba` -> `fat32::mount` -> read
    //   `SHELL.BIN`.
    // - A legacy BIOS boot always has an ATA disk (including the BIOS+VBE
    //   graphics path), so it reads `SHELL.BIN` from the FAT32 superfloppy
    //   (VBR at LBA 0) over ATA PIO.
    //
    // `primary_present()` distinguishes the two without a dedicated boot-source
    // flag, and is callable before `drivers::ata::init()`.
    let uefi =
        booted_via_framebuffer(boot_info_raw, has_boot_info) && !drivers::ata::primary_present();

    let shell_image = if uefi {
        let bi = boot_info::BootInfo::get().unwrap();
        let fb = bi.fb_info;

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
            let _ = writeln!(console, "Loading SHELL.BIN from the ESP via AHCI...");
        });

        // Reach the FAT32 EFI System Partition through the AHCI controller.
        drivers::ahci::init();
        drivers::block::init_ahci();

        let esp_lba = io::gpt::find_esp_start_lba().expect("ESP not found on GPT disk");
        debugln!("ESP Start LBA: {}", esp_lba);

        let vol = io::fat32::Fat32Volume::mount(esp_lba).expect("FAT32 ESP mount failed");
        io::vfs::mount(alloc::boxed::Box::new(io::fat32::Fat32Fs::new(vol)));

        let image = io::vfs::read_file("shell.bin").expect("failed to read SHELL.BIN from ESP");
        debugln!("Loaded SHELL.BIN from ESP: {} bytes", image.len());

        crate::console::with_console(|console| {
            let _ = writeln!(
                console,
                "Loaded SHELL.BIN ({} bytes). Starting...",
                image.len()
            );
        });

        image
    } else {
        // Legacy BIOS path: the shell lives on a FAT32 superfloppy (VBR at LBA 0)
        // reached via ATA PIO. This now uses the same read-only FAT32 backend as the
        // UEFI path; the only differences are the transport (ATA) and part_lba (0).
        drivers::ata::init();
        drivers::block::init_ata();
        debugln!("ATA PIO driver initialized");

        let vol = io::fat32::Fat32Volume::mount(0).expect("FAT32 mount (ATA, LBA0) failed");
        io::vfs::mount(alloc::boxed::Box::new(io::fat32::Fat32Fs::new(vol)));
        debugln!("FAT32 file system mounted (ATA, LBA0)");

        io::vfs::read_file("shell.bin").expect("failed to load SHELL.BIN from FAT32")
    };

    // #63 verification banner: surface the ACTIVE page-table model on the visible
    // console (framebuffer/VGA) right before the shell starts, so a boot can be
    // confirmed to run the kernel-owned tables without reading the serial log. Keyed on
    // the same `BootInfo`-published fact `vmm::init` uses to pick its path.
    crate::console::with_console(|console| {
        if boot_info_published {
            let _ = writeln!(
                console,
                ">> #63: KERNEL-OWNED page tables ACTIVE (CR3 switched to kernel-built direct map)"
            );
        } else {
            let _ = writeln!(
                console,
                ">> #63: firmware-clone page tables (no BootInfo - fallback path)"
            );
        }
    });

    // --- Shared scheduler bring-up (both boot paths) ---

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

    // Spawn the user-space shell task from the image loaded above (FAT32 on both
    // paths: ESP via AHCI on UEFI, superfloppy at LBA 0 via ATA on legacy BIOS).
    //
    // The shell is the only task granted the privileged-syscall capability at
    // spawn time (M6, `docs/CODE_REVIEW_2026-07-23.md`): it is the sole
    // legitimate caller of the `Shutdown` syscall. Every task spawned later
    // via `Exec` defaults to unprivileged (see `process::exec_from_vfs`).
    let shell_pid = process::exec_from_image(&shell_image, true)
        .expect("failed to spawn SHELL.BIN user-mode task");

    // On the UEFI path there is no serial console on real hardware, so leave a
    // visible breadcrumb on the framebuffer. If boot stalls after "Starting...",
    // whether these lines appear localizes the failure: missing => exec/mapping
    // faulted; present but no shell => the scheduler never preempted (timer/IRQ).
    if uefi {
        crate::console::with_console(|console| {
            let _ = writeln!(
                console,
                "Shell mapped (PID {}). Starting scheduler...",
                shell_pid
            );
        });
    }

    scheduler::start();
    debugln!(
        "Scheduler started with keyboard worker + SHELL.BIN (PID {})",
        shell_pid
    );

    if uefi {
        crate::console::with_console(|console| {
            let _ = writeln!(console, "Scheduler running, awaiting shell...");
        });
    }

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
fn booted_via_framebuffer(_boot_info_raw: u64, has_boot_info: bool) -> bool {
    if !has_boot_info {
        return false;
    }
    let bi = boot_info::BootInfo::get().unwrap();
    bi.video_type == boot_info::VideoModeType::Framebuffer && bi.fb_info.base_address != 0
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
    let bi = boot_info::BootInfo::get().unwrap();
    let fb = bi.fb_info;
    if fb.base_address == 0 || fb.size == 0 {
        return;
    }

    // Configure PAT MSR (0x277) to set PAT1 (bits 8..15) to Write-Combining (0x01).
    // The default value is usually 0x0007_0406_0007_0406 (PAT1 = 0x04 = WT).
    // SAFETY:
    // - Repointing PAT1 to WC is only safe because it is the sole PAT slot ever
    //   repurposed by this kernel: `map_virtual_to_physical_wc`
    //   (memory/vmm/mapping.rs) is the only mapping function that sets PWT=1/PCD=0
    //   on a leaf entry, and it is only ever called for framebuffer pages here.
    //   No other mapping in the kernel selects PAT1 expecting the default WT
    //   semantics, so redefining its meaning cannot silently change the caching
    //   behavior of an unrelated mapping.
    // - The MSR write itself is architecturally valid only from ring 0, which this
    //   code always runs in.
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

    // Publish that the framebuffer is now safe to write to. The panic handler reads
    // this flag before touching `fb.base_address`; setting it only here (after every
    // page above has been mapped) is what prevents an early panic from triple-faulting
    // by writing through a still-unmapped physical address. `Release` pairs with the
    // `Relaxed` load in the panic path: the flag transitions only `false -> true`, so
    // any observer either sees the fully-mapped framebuffer or safely falls back.
    boot_info::FRAMEBUFFER_MAPPED.store(true, core::sync::atomic::Ordering::Release);
}
