//! Scroll dirty-range narrowing test (issue #62, finding L12).
//!
//! Before this fix, `FramebufferConsole::scroll()` marked the *entire*
//! framebuffer height dirty on every scroll, regardless of how many scanlines
//! the row-shift + bottom-row-clear actually touched. When the framebuffer's
//! height is not an exact multiple of the glyph height (`GLYPH_H` = 16), there
//! are leftover scanlines below the last full text row that `scroll()` never
//! writes to. Marking those dirty forced `take_flush_job`'s RAM-to-RAM
//! backbuffer -> scratch copy — which still runs under `GLOBAL_CONSOLE`'s
//! lock (see issue #16 / `console_flush_lock_test.rs`) — to cover the whole
//! screen instead of just the changed region.
//!
//! This test exercises the pure arithmetic seam
//! `FramebufferConsole::scroll_dirty_range_for_test`, which computes exactly
//! the scanline range `scroll()` now marks dirty, without needing a live
//! console or framebuffer.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use kaos_kernel::console::FramebufferConsole;

/// Entry point for the scroll dirty-range test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

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

/// Contract: for a console with `rows` text rows (each `GLYPH_H` = 16
/// scanlines tall), a scroll's dirty range must span exactly `rows * 16`
/// scanlines starting at `0` — the `rows - 1` shifted rows plus the newly
/// cleared bottom row — not the framebuffer's full height.
/// Given: A handful of representative row counts.
/// When: `scroll_dirty_range_for_test` computes the dirty range for each.
/// Then: The returned range is exactly `(0, rows * 16 - 1)`.
/// Failure Impact: A regression here means `scroll()` once again marks
/// scanlines it never wrote to as dirty, forcing the locked RAM-to-RAM
/// snapshot copy in `take_flush_job` to cover more of the screen than
/// actually changed (issue #62 / L12).
#[test_case]
fn test_scroll_dirty_range_matches_touched_rows() {
    kaos_kernel::test_assert!(
        FramebufferConsole::scroll_dirty_range_for_test(3) == (0, 47),
        "3 text rows * 16 scanlines/row = 48 scanlines (y = 0..=47)"
    );
    kaos_kernel::test_assert!(
        FramebufferConsole::scroll_dirty_range_for_test(25) == (0, 399),
        "25 text rows * 16 scanlines/row = 400 scanlines (y = 0..=399)"
    );
    kaos_kernel::test_assert!(
        FramebufferConsole::scroll_dirty_range_for_test(1) == (0, 15),
        "1 text row * 16 scanlines/row = 16 scanlines (y = 0..=15)"
    );
}

/// Contract: the dirty range must never depend on the framebuffer's raw pixel
/// height — only on the number of text rows. This is exactly the behavior
/// change from the old code, which used to mark `(0, fb.height - 1)` dirty
/// even when `fb.height` included scanlines below the last full text row that
/// no glyph write ever touches.
/// Given: A framebuffer height that is *not* an exact multiple of `GLYPH_H`
///   (e.g. 53 px for 3 rows of 16 px = 48 px, leaving 5 leftover scanlines).
/// When: The dirty range for those 3 rows is computed.
/// Then: The range stops at `47`, not `52` — the leftover scanlines below the
///   last full text row are excluded.
/// Failure Impact: Without this narrowing, every scroll needlessly copies and
/// (eventually) uploads scanlines that are never rendered to, which is
/// exactly the residual full-screen locked copy described in issue #62 / L12.
#[test_case]
fn test_scroll_dirty_range_excludes_leftover_scanlines_below_last_row() {
    let rows = 3;
    let fb_height_with_leftover_scanlines: u32 = rows as u32 * 16 + 5;

    let (start, end) = FramebufferConsole::scroll_dirty_range_for_test(rows);

    kaos_kernel::test_assert!(start == 0, "dirty range must start at scanline 0");
    kaos_kernel::test_assert!(
        end == 47,
        "dirty range must stop at the last scanline of the last full text row (47), \
         not extend into the leftover scanlines below it"
    );
    kaos_kernel::test_assert!(
        end < fb_height_with_leftover_scanlines - 1,
        "dirty range must be strictly narrower than (fb.height - 1) when the \
         framebuffer height is not an exact multiple of GLYPH_H"
    );
}
