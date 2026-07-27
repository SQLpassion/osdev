//! Contract test for the C1 fix: the panic path must never treat the linear
//! framebuffer as a live, writable pointer before it has actually been mapped.
//!
//! # Background
//!
//! `BOOT_INFO_PTR` is published in `KernelMain` right after the boot-info magic
//! check succeeds — before `gdt::init()`, `fpu::init()`, `pmm::init()`,
//! `interrupts::init()`, `vmm::init()`, `heap::init()`, or `map_framebuffer()` run.
//! `BootInfo.video_type` is set by the bootloader, not the kernel, so on a BIOS+VBE
//! boot it already reads `Framebuffer` at the moment `BOOT_INFO_PTR` is published.
//!
//! A panic between that publish and `map_framebuffer()` running (e.g. from
//! `pmm::init()` hitting a malformed memory map) must not pick the framebuffer
//! writer: on BIOS+VBE the reported base address lives outside the bootstrap
//! loader's low identity map, and no page-fault/IDT handler exists yet, so writing
//! through it would triple-fault the CPU with zero diagnostic output.
//!
//! # Test seam
//!
//! `panic.rs` (and its `PanicFramebufferWriter`) is intentionally excluded from the
//! `kaos_kernel` library target (see `kernel/src/lib.rs`): it defines the crate's
//! `#[panic_handler]`, and every integration test binary supplies its own — pulling
//! `panic.rs` into the library would collide with that. So this test exercises the
//! shared predicate the panic path is built on,
//! `kaos_kernel::boot_info::framebuffer_panic_writer_available()`, which is the exact
//! gate `PanicFramebufferWriter::from_boot_info()` checks first. That function is a
//! pure combination of `FRAMEBUFFER_MAPPED` and the published `BootInfo`, with no
//! hardware/heap dependency, making it the most direct testable seam for this fix.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use kaos_kernel::boot_info::{
    framebuffer_panic_writer_available, BootInfo, FramebufferInfo, PixelFormat, VideoModeType,
    BOOT_INFO_PTR, FRAMEBUFFER_MAPPED,
};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Builds a `BootInfo` describing a BIOS+VBE-style framebuffer boot: a valid magic,
/// `video_type == Framebuffer`, and a non-zero `fb_info.base_address`. This mirrors
/// what the bootloader publishes *before* the kernel has mapped anything.
fn framebuffer_boot_info() -> BootInfo {
    BootInfo {
        magic: 0x4B41_4F53_5F42_4F4F,
        video_type: VideoModeType::Framebuffer,
        fb_info: FramebufferInfo {
            base_address: 0xE000_0000,
            size: 0x40_0000,
            width: 1024,
            height: 768,
            pixels_per_scanline: 1024,
            pixel_format: PixelFormat::Bgr,
        },
        memory_map_addr: 0,
        memory_map_len: 0,
        kernel_size: 0,
        pmm_metadata_base: 0,
        pmm_metadata_size: 0,
        boot_year: 0,
        boot_month: 0,
        boot_day: 0,
        boot_hour: 0,
        boot_minute: 0,
        boot_second: 0,
        boot_timezone: 0,
    }
}

/// Restores the two globals under test to their pre-test state so this test does not
/// leak state into any test that runs after it in the same binary.
fn reset_globals(saved_ptr: u64, saved_mapped: bool) {
    BOOT_INFO_PTR.store(saved_ptr, Ordering::Release);
    FRAMEBUFFER_MAPPED.store(saved_mapped, Ordering::Relaxed);
}

/// Contract: even when `BootInfo.video_type == Framebuffer` (as it already is on a
/// BIOS+VBE boot the instant `BOOT_INFO_PTR` is published), the gate must return
/// `false` while `FRAMEBUFFER_MAPPED` is still `false` — i.e. before `map_framebuffer()`
/// has run. This is exactly the window in which `pmm::init()`, `vmm::init()`, etc. can
/// panic.
/// Failure Impact: the panic handler would pick the framebuffer writer before the
/// framebuffer is mapped, writing through an unmapped physical address and
/// triple-faulting the CPU with zero diagnostic output (C1 / #41). Release-blocking.
#[test_case]
fn test_gate_false_when_video_type_framebuffer_but_not_mapped() {
    let saved_ptr = BOOT_INFO_PTR.load(Ordering::Acquire);
    let saved_mapped = FRAMEBUFFER_MAPPED.load(Ordering::Relaxed);

    let boot_info = framebuffer_boot_info();
    let ptr = &boot_info as *const BootInfo as u64;

    // SAFETY: `boot_info` is a live local on this function's stack; this mirrors
    // `KernelMain` publishing `BOOT_INFO_PTR` right after the magic check, before
    // `map_framebuffer()` has ever run. The pointer is restored below before this
    // stack frame returns.
    BOOT_INFO_PTR.store(ptr, Ordering::Release);
    FRAMEBUFFER_MAPPED.store(false, Ordering::Relaxed);

    let available = framebuffer_panic_writer_available();

    reset_globals(saved_ptr, saved_mapped);

    kaos_kernel::test_assert!(
        !available,
        "framebuffer_panic_writer_available() must be false before map_framebuffer() runs, \
         even though video_type already reads Framebuffer"
    );
}

/// Contract: once `FRAMEBUFFER_MAPPED` is `true` (i.e. `map_framebuffer()` has run to
/// completion) and `BootInfo` describes a valid framebuffer, the gate must return
/// `true` so the panic path can render diagnostics to VRAM instead of falling back
/// to VGA text mode.
/// Failure Impact: a regression here would either re-introduce the triple-fault (if
/// the gate stayed stuck at `false`) or mask that the fallback path is the only one
/// ever exercised. Release-blocking.
#[test_case]
fn test_gate_true_once_mapped() {
    let saved_ptr = BOOT_INFO_PTR.load(Ordering::Acquire);
    let saved_mapped = FRAMEBUFFER_MAPPED.load(Ordering::Relaxed);

    let boot_info = framebuffer_boot_info();
    let ptr = &boot_info as *const BootInfo as u64;

    // SAFETY: see `test_gate_false_when_video_type_framebuffer_but_not_mapped`; same
    // stack-local publish/restore discipline.
    BOOT_INFO_PTR.store(ptr, Ordering::Release);
    FRAMEBUFFER_MAPPED.store(true, Ordering::Release);

    let available = framebuffer_panic_writer_available();

    reset_globals(saved_ptr, saved_mapped);

    kaos_kernel::test_assert!(
        available,
        "framebuffer_panic_writer_available() must be true once map_framebuffer() has \
         published FRAMEBUFFER_MAPPED and BootInfo describes a valid framebuffer"
    );
}

/// Contract: when no `BootInfo` has been published at all (`BOOT_INFO_PTR == 0`), the
/// gate must be `false` regardless of `FRAMEBUFFER_MAPPED` — there is nothing to read
/// `fb_info` from.
/// Failure Impact: a null/garbage read of `BootInfo` on a boot path without a unified
/// boot-info block (e.g. the legacy `KernelMain(kernel_size)` compatibility path).
/// Release-blocking.
#[test_case]
fn test_gate_false_when_no_boot_info_published() {
    let saved_ptr = BOOT_INFO_PTR.load(Ordering::Acquire);
    let saved_mapped = FRAMEBUFFER_MAPPED.load(Ordering::Relaxed);

    BOOT_INFO_PTR.store(0, Ordering::Release);
    FRAMEBUFFER_MAPPED.store(true, Ordering::Relaxed);

    let available = framebuffer_panic_writer_available();

    reset_globals(saved_ptr, saved_mapped);

    kaos_kernel::test_assert!(
        !available,
        "framebuffer_panic_writer_available() must be false when no BootInfo has been \
         published, even if FRAMEBUFFER_MAPPED is stale-true"
    );
}
