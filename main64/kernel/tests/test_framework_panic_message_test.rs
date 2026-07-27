//! Regression test for H2 (issue #43): the test-framework panic handler must
//! not drop interpolated panic messages.
//!
//! `test_panic_handler` in `kernel/src/testing.rs` used to print the panic
//! message only via `PanicMessage::as_str()`, which returns `Some` only for a
//! bare string-literal panic with no format arguments. Every real assertion
//! failure produced by `test_assert_eq!`/`test_assert!` panics with
//! interpolated arguments (e.g. `panic!("... {:?} ...", left, right)`), so the
//! diagnostic message was silently skipped for essentially every real test
//! failure - defeating the purpose of the assertion macros.
//!
//! This binary cannot invoke `test_panic_handler` itself and then inspect its
//! `debugln!` output from `cargo test`: the harness only observes the QEMU
//! exit code produced by `tests/test_runner.sh`, not serial content. Instead
//! this test exercises the exact seam the fix relies on: formatting
//! `PanicInfo::message()` via `Display` (as `test_panic_handler` now does)
//! rather than via `Option::as_str()` (the old, buggy behavior). We
//! deliberately trigger a panic the same way `test_assert_eq!` does - with
//! interpolated arguments - and use `panic_message_contains` (which already
//! formats via `Display`) to confirm the interpolated values survive.
//! Under the pre-fix logic, an `as_str()`-based check on this same panic
//! would have observed `None` and dropped the message entirely.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();

    // If this is reached, the expected panic did not happen.
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Both interpolated values must be present in the `Display`-formatted
    // message. A `.as_str()`-based check (the pre-fix behavior of
    // `test_panic_handler`) would see `None` for this panic, since it was
    // built with format arguments - making this contract unverifiable and
    // silently dropping the diagnostic in the real failure path.
    //
    // `panic_message_contains` scans one formatter `write_str` chunk at a
    // time (see its doc comment in `testing.rs`) and does not stitch
    // adjacent chunks together, so a search string that straddles a chunk
    // boundary - e.g. static text immediately followed by a `{:?}`-formatted
    // argument - can miss even though the full message contains it. To keep
    // this test independent of that separately-tracked limitation, we search
    // only for each interpolated numeric literal on its own, which is
    // rendered as a single `write_str` chunk by the integer `Debug` impl.
    let has_left = kaos_kernel::testing::panic_message_contains(info, "246813579");
    let has_right = kaos_kernel::testing::panic_message_contains(info, "987654321");

    if has_left && has_right {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: a panic constructed with format arguments - exactly how
/// `test_assert_eq!` builds its panic on failure - retains the interpolated
/// values when the message is rendered via `Display`.
/// Given: `test_assert_eq!` is invoked with two unequal, distinctive values.
/// When: the assertion fails and panics with an interpolated message.
/// Then: the panic message, read via `Display` (as `test_panic_handler` now
/// does), contains both interpolated values instead of being dropped, as it
/// would be with the pre-fix `as_str()`-based check.
#[test_case]
fn test_formatted_assertion_panic_retains_interpolated_values() {
    kaos_kernel::test_assert_eq!(246813579, 987654321);
}
