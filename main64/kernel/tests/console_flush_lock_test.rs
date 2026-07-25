//! Console VRAM-flush lock-scope integration tests (issue #16).
//!
//! Before the fix, `console::with_console` held `GLOBAL_CONSOLE`'s
//! interrupt-disabling spinlock for the *entire* duration of the closure
//! passed to it, and `FramebufferConsole`'s `KernelConsole` methods performed
//! the (potentially full-screen) VRAM `copy_nonoverlapping` from *inside*
//! that closure. Any operation that marks the whole screen dirty (most
//! notably `FramebufferConsole::scroll`, which marks every scanline dirty
//! before returning) therefore caused a full-screen MMIO blit to run with
//! interrupts disabled — a latency spike that can stall the scheduler and
//! other interrupts.
//!
//! These tests exercise the real `console::init` / `console::with_console`
//! path against a fake linear framebuffer (a heap buffer standing in for
//! physical VRAM) and use the test-only introspection hook
//! `console::interface::last_flush_ran_with_interrupts_enabled` to verify
//! that the deferred VRAM upload now runs *after* `GLOBAL_CONSOLE`'s lock has
//! been released, not while interrupts are still disabled by it.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use kaos_kernel::arch::interrupts;
use kaos_kernel::boot_info::{
    BootInfo, FramebufferInfo, PixelFormat, VideoModeType, BOOT_INFO_PTR,
};
use kaos_kernel::console;
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the console flush-lock integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Small test-only framebuffer geometry: 10 text columns x 3 text rows
/// (matching `FramebufferConsole`'s fixed 8x16 glyph size).
const TEST_COLS: usize = 10;
const TEST_ROWS: usize = 3;
const TEST_WIDTH: u32 = (TEST_COLS * 8) as u32;
const TEST_HEIGHT: u32 = (TEST_ROWS * 16) as u32;

/// RGB value of `Color::White` (`0x00FFFFFF`), which is identical regardless
/// of RGB/BGR channel order, used below to confirm that rendered glyph pixels
/// actually made it into the fake VRAM buffer.
const WHITE_RGB: u32 = 0x00FF_FFFF;

/// Sets up a fake linear framebuffer backed by a heap buffer, installs it as
/// the active `BootInfo`, initializes the console module in framebuffer mode,
/// and runs `body` with mutable access to the fake VRAM contents.
///
/// On return, the console is switched back to VGA text mode and `BOOT_INFO_PTR`
/// is cleared *before* the fake VRAM buffer is dropped, so no dangling
/// pointer into freed heap memory is ever left reachable from
/// `GLOBAL_CONSOLE` or `BOOT_INFO_PTR` for later tests in this binary.
fn with_test_framebuffer_console(body: impl FnOnce(&mut Vec<u32>)) {
    let mut vram: Vec<u32> = vec![0u32; (TEST_WIDTH * TEST_HEIGHT) as usize];

    let boot_info = BootInfo {
        magic: 0,
        video_type: VideoModeType::Framebuffer,
        fb_info: FramebufferInfo {
            base_address: vram.as_mut_ptr() as u64,
            size: vram.len() * core::mem::size_of::<u32>(),
            width: TEST_WIDTH,
            height: TEST_HEIGHT,
            pixels_per_scanline: TEST_WIDTH,
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
    };

    // SAFETY:
    // - `boot_info` lives on this function's stack frame, which stays alive
    //   for the entire duration of `body`, i.e. for as long as anything could
    //   possibly still read `BOOT_INFO_PTR` on this synchronous, single-core
    //   test path.
    // - `console::init` only dereferences the pointer synchronously while
    //   constructing the new `FramebufferConsole`; it does not retain it.
    BOOT_INFO_PTR.store(&boot_info as *const BootInfo as u64, Ordering::Relaxed);
    console::init(VideoModeType::Framebuffer);

    body(&mut vram);

    // Switch back to VGA text mode (dropping the `FramebufferConsole`, whose
    // stored `fb_info.base_address` would otherwise dangle once `vram` below
    // is freed) and clear the boot-info pointer, so nothing is left pointing
    // at this function's soon-to-be-freed stack/heap memory.
    console::init(VideoModeType::VgaText);
    BOOT_INFO_PTR.store(0, Ordering::Relaxed);
}

/// Contract: a deferred flush triggered by a full-screen dirty range (as
/// produced by `FramebufferConsole::scroll`) is applied by `with_console`
/// *after* releasing `GLOBAL_CONSOLE`'s lock, so it runs with interrupts
/// enabled rather than while the lock still has them disabled.
/// Given: A fake framebuffer console with interrupts enabled beforehand.
/// When: Enough lines are printed through `with_console` to force a scroll,
///   which marks the entire screen dirty.
/// Then: The recorded interrupt state at flush time must be "enabled", and
///   the fake VRAM buffer must contain the actually-rendered glyph pixels.
/// Failure Impact: A regression here means the console once again performs a
///   large MMIO blit while interrupts are disabled (issue #16), reintroducing
///   the scheduler/interrupt latency spike.
#[test_case]
fn test_scroll_triggered_flush_runs_with_interrupts_enabled() {
    with_test_framebuffer_console(|vram| {
        interrupts::enable();
        assert!(
            interrupts::are_enabled(),
            "precondition: interrupts must be enabled before the call"
        );

        console::with_console(|c| {
            c.clear();
            // TEST_ROWS + 1 newlines push the cursor one row past the bottom,
            // forcing FramebufferConsole::scroll() to run and mark the
            // *entire* screen dirty before with_console returns.
            for _ in 0..=TEST_ROWS {
                c.print_str("X\n");
            }
        });

        assert!(
            interrupts::are_enabled(),
            "with_console must restore the caller's original (enabled) \
             interrupt state after the call"
        );
        assert!(
            console::interface::last_flush_ran_with_interrupts_enabled(),
            "the deferred VRAM flush triggered by a full-screen dirty range \
             (scroll) must run with interrupts enabled, i.e. after \
             GLOBAL_CONSOLE's lock has been released, not while it still \
             has interrupts disabled"
        );
        assert!(
            vram.contains(&WHITE_RGB),
            "the flush must have actually copied rendered glyph pixels into \
             the (fake) physical framebuffer, not merely been recorded as a \
             no-op"
        );

        interrupts::disable();
    });
}

/// Contract: `with_console` never force-enables interrupts around the
/// deferred flush; if the caller already had interrupts disabled, the flush
/// still runs (and the console content is still correct), but with
/// interrupts left disabled throughout.
/// Given: A fake framebuffer console with interrupts explicitly disabled
///   beforehand.
/// When: Enough lines are printed through `with_console` to force a scroll.
/// Then: The recorded interrupt state at flush time must be "disabled", and
///   interrupts remain disabled after `with_console` returns, while the
///   flush still visibly ran (VRAM contains rendered pixels).
/// Failure Impact: A regression here would mean `with_console` unexpectedly
///   changes the caller's interrupt state, which could re-enable interrupts
///   inside another lock's critical section and reintroduce a deadlock risk.
#[test_case]
fn test_flush_does_not_force_enable_interrupts_when_caller_had_them_disabled() {
    with_test_framebuffer_console(|vram| {
        interrupts::disable();
        assert!(
            !interrupts::are_enabled(),
            "precondition: interrupts must be disabled before the call"
        );

        console::with_console(|c| {
            c.clear();
            for _ in 0..=TEST_ROWS {
                c.print_str("Y\n");
            }
        });

        assert!(
            !interrupts::are_enabled(),
            "with_console must restore the caller's original (disabled) \
             interrupt state after the call"
        );
        assert!(
            !console::interface::last_flush_ran_with_interrupts_enabled(),
            "with_console must not force-enable interrupts around the \
             deferred flush when the caller already had them disabled"
        );
        assert!(
            vram.contains(&WHITE_RGB),
            "the flush must still have applied even though interrupts were \
             disabled throughout"
        );
    });
}
