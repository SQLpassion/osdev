//! #63 Phase 5 regression: the kernel heap and the VGA text page work through the
//! *rebuilt* (narrowed) PML4 slot 256.
//!
//! Phase 5 stops slot 256 from being a verbatim low-RAM mirror: the kernel image is
//! mapped per-section, and the kernel heap (higher-half) is no longer eagerly backed —
//! it is served by the page-fault demand pager (`page_fault.rs`'s kernel-heap-arena
//! path). That path was effectively dead before Phase 5 (the mirror satisfied every heap
//! access), so this test makes it live: it publishes the real loader `BootInfo` so
//! `vmm::init` takes the kernel-owned switch, then drives real heap allocation (including
//! a 64 KiB block spanning many pages, the task-stack size) with write-then-read
//! verification, and writes/reads the eagerly-mapped VGA text page.
//!
//! Requirement: must be booted by a loader that publishes a `BootInfo` (BIOS/UEFI). On a
//! BootInfo-less boot `vmm::init` falls back to the firmware clone and this exercises the
//! old mirror path instead — still valid, just not the Phase 5 path.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use kaos_kernel::arch::{cpu, interrupts, msr};
use kaos_kernel::boot_info::BOOT_INFO_PTR;
use kaos_kernel::memory::{heap, pmm, vmm};

const BOOT_INFO_MAGIC: u64 = 0x4B41_4F53_5F42_4F4F; // "KAOS_BOO"

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(boot_info_raw: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // SAFETY: magic-check before trusting the loader pointer; publishing it makes
    // `vmm::init` take the kernel-owned-table switch (narrowed slot 256).
    let has_boot_info =
        boot_info_raw != 0 && unsafe { *(boot_info_raw as *const u64) } == BOOT_INFO_MAGIC;
    if has_boot_info {
        BOOT_INFO_PTR.store(boot_info_raw, Ordering::Release);
    }

    msr::enable_no_execute();
    pmm::init(false);
    interrupts::init();
    vmm::init(false); // BootInfo published => switch to kernel-owned table (RO+X .text)
    heap::init(false); // first heap writes now go through demand paging, not the mirror
    cpu::enable_write_protect();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: heap allocations of several sizes — including a 64 KiB block (the ring-3
/// task-stack size) that spans 16 pages — are writable and read back correctly through
/// the rebuilt slot 256 (i.e. the kernel-heap demand-paging path works).
/// Failure Impact: a broken demand-paging path under the narrowed slot 256 would fault or
/// corrupt every heap allocation on a real (BootInfo) boot — release-blocking.
#[test_case]
fn test_heap_demand_paging_through_rebuilt_slot256() {
    for &size in &[16usize, 4096, 64 * 1024] {
        let ptr = heap::malloc(size);
        assert!(!ptr.is_null(), "malloc({size}) must not return null");

        // Touch one byte per 4 KiB page (write a size-dependent marker), then read back.
        let marker = (size as u8) ^ 0xA5;
        let mut off = 0usize;
        while off < size {
            // SAFETY: `ptr` owns `size` bytes; `off < size` and page-strided.
            unsafe { core::ptr::write_volatile(ptr.add(off), marker) };
            off += 4096;
        }
        let mut off = 0usize;
        while off < size {
            // SAFETY: same range just written.
            let got = unsafe { core::ptr::read_volatile(ptr.add(off)) };
            assert_eq!(got, marker, "heap byte at +{off} must read back");
            off += 4096;
        }

        heap::free(ptr);
    }
}

/// Contract: the VGA text page (higher-half `0xFFFF_8000_000B_8000`, phys `0xB8000`) is
/// eagerly mapped RW by the rebuilt slot 256 — it cannot be demand-paged (the #PF handler
/// refuses non-heap higher-half faults), and the fatal-exception/panic paths write it.
/// Failure Impact: an unmapped VGA page would triple-fault on the first fatal exception —
/// release-blocking.
#[test_case]
fn test_vga_text_page_is_writable_after_switch() {
    let vga = 0xFFFF_8000_000B_8000 as *mut u16;
    // A VGA text cell: 0x0F00 | 'K'. Write then read back (RAM-backed at phys 0xB8000).
    let cell: u16 = 0x0F00 | u16::from(b'K');
    // SAFETY: eagerly mapped RW+NX by map_kernel_image_higher_half; valid for one cell.
    unsafe {
        core::ptr::write_volatile(vga, cell);
        assert_eq!(
            core::ptr::read_volatile(vga),
            cell,
            "VGA cell must read back"
        );
    }
}
