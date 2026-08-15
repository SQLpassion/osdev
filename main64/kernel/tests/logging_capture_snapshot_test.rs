//! Regression tests for issue #54: `print_captured_target` reads the capture
//! buffer after releasing the logger lock.
//!
//! Before the fix, `logging::print_captured_target` snapshotted only a
//! `(ptr, len)` pair under `LOGGER`'s lock, then read *through* that raw
//! pointer after releasing it. `capture_buf` is a fixed-size array embedded
//! in `LogState` and is never reallocated, so the pointer itself stayed
//! valid — but nothing stopped another task (woken via a timer interrupt,
//! since `capture_target_line` can run again the moment the lock is free)
//! from appending to, or resetting via `set_capture_enabled`, that same live
//! buffer while the comparatively slow, multi-line screen-formatting loop
//! was still reading through it. That could garble/truncate the dump, or
//! make the UTF-8 decode fail and silently return nothing.
//!
//! The fix copies the buffer's valid bytes into an owned `Vec<u8>` while
//! still holding the lock (`logging::capture_snapshot`, exposed here for
//! testing as `capture_snapshot_for_test`), then formats that owned copy
//! unlocked (`format_captured_lines`, exposed here as
//! `format_captured_for_test`). These tests exercise exactly the race
//! described above: take a snapshot of known content, mutate the *live*
//! buffer the way a preempting task would, and confirm the already-taken
//! snapshot's rendered output still reflects the original content only.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::string::String;
use core::panic::PanicInfo;

use kaos_kernel::arch::interrupts;
use kaos_kernel::boot_info::VideoModeType;
use kaos_kernel::console;
use kaos_kernel::logging;
use kaos_kernel::memory::{heap, pmm, vmm};

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

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// VGA text-mode geometry, matching `screen_test.rs`.
const VGA_BUFFER: usize = 0xFFFF8000000B8000;
const VGA_COLS: usize = 80;
const VGA_ROWS: usize = 25;

/// Reads back the entire VGA text-mode buffer's character bytes (skipping
/// the interleaved color-attribute bytes) into a single `String`, so tests
/// can assert on rendered content with a plain `contains` check instead of
/// tracking exact cursor/row/column positions.
fn vga_text_snapshot() -> String {
    let mut out = String::with_capacity(VGA_ROWS * VGA_COLS);
    for row in 0..VGA_ROWS {
        for col in 0..VGA_COLS {
            let cell = VGA_BUFFER + ((row * VGA_COLS + col) * 2);
            // SAFETY:
            // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
            // - `cell` points to VGA text MMIO for an in-bounds (row, col) cell.
            // - Volatile read is required for MMIO.
            let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
            out.push(ch as char);
        }
    }
    out
}

/// Resets the VGA console and the logger's capture buffer to a known,
/// isolated starting state before each test below runs.
fn reset_console_and_capture() {
    console::init(VideoModeType::VgaText);
    console::with_console(|c| c.clear());
    logging::set_capture_enabled(true);
}

/// Contract: formatting an already-taken capture snapshot must render only
/// the content that was present at snapshot time, even if the live capture
/// buffer is subsequently appended to and reset (as a preempting task could
/// do between the lock-protected snapshot and the unlocked formatting pass).
/// Given: The capture buffer holds one known line for target `csnap54a`.
/// When: A snapshot is taken (`capture_snapshot_for_test`), the live buffer
///   is then mutated with different content for the same target, and the
///   *original* snapshot is formatted (`format_captured_for_test`).
/// Then: The rendered output must contain the original line and must not
///   contain the content written after the snapshot was taken.
/// Failure Impact: A regression here reintroduces issue #54 — the dump could
///   read torn or entirely different data than what was captured, because
///   formatting would once again be reading the mutable live buffer instead
///   of an owned copy.
#[test_case]
fn test_formatting_a_snapshot_ignores_later_mutations_of_the_live_buffer() {
    reset_console_and_capture();

    // Step 1: Capture one known line, then take an owned snapshot of it
    // while still (implicitly) holding the logger lock, exactly as
    // `print_captured_target` does internally.
    logging::logln_with_options("csnap54a", format_args!("line-one-original"), false, true);
    let (snapshot_bytes, snapshot_overflow) = logging::capture_snapshot_for_test();
    assert!(
        core::str::from_utf8(&snapshot_bytes)
            .unwrap()
            .contains("csnap54a|line-one-original\n"),
        "sanity: snapshot must contain the line captured before it was taken"
    );

    // Step 2: Simulate a task preempting the (real) dump between the
    // snapshot and the formatting pass: reset capture and log different
    // content under the same target.
    logging::set_capture_enabled(true);
    logging::logln_with_options("csnap54a", format_args!("line-two-MUTATED"), false, true);

    // Sanity check: confirm the live buffer really did change, so the
    // assertion below is actually exercising snapshot isolation and not
    // trivially passing because nothing changed.
    let (live_bytes, _) = logging::capture_snapshot_for_test();
    assert!(
        core::str::from_utf8(&live_bytes)
            .unwrap()
            .contains("line-two-MUTATED"),
        "sanity: the live capture buffer must reflect the post-snapshot mutation"
    );

    // Step 3: Format the *original* snapshot (taken before the mutation).
    // Its rendered output must be unaffected by the later mutation.
    console::with_console(|screen| {
        logging::format_captured_for_test(
            &snapshot_bytes,
            "csnap54a",
            snapshot_overflow,
            screen,
            |_| false,
        );
    });

    let rendered = vga_text_snapshot();
    assert!(
        rendered.contains("line-one-original"),
        "formatting a snapshot must render the content captured at snapshot time"
    );
    assert!(
        !rendered.contains("line-two-MUTATED"),
        "formatting a snapshot must not be affected by mutations made to the \
         live capture buffer after the snapshot was taken"
    );
}

/// Contract: `print_captured_target` (the public entry point) still dumps
/// whatever is currently captured for a target under normal, non-racing
/// use — i.e. the snapshot-based fix does not change ordinary behavior.
/// Given: A known line is captured for a distinct target.
/// When: `print_captured_target` is called for that target.
/// Then: The rendered output contains the captured message.
/// Failure Impact: A regression here would mean the fix broke the ordinary
/// (non-racing) capture-and-dump path relied upon by callers such as
/// `memory::vmm`'s console debug dump.
#[test_case]
fn test_print_captured_target_renders_captured_content() {
    reset_console_and_capture();

    logging::logln_with_options("csnap54b", format_args!("hello-from-capture"), false, true);

    console::with_console(|screen| {
        logging::print_captured_target(screen, "csnap54b", |_| false);
    });

    let rendered = vga_text_snapshot();
    assert!(
        rendered.contains("hello-from-capture"),
        "print_captured_target must render previously captured content for its target"
    );
}
