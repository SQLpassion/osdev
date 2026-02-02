//! Test Framework for KAOS Kernel
//!
//! This module provides a custom test framework for running tests in a bare-metal
//! environment. Tests are run inside QEMU and results are output via serial port.
//!
//! Each integration test file (in `tests/`) must enable the custom test framework
//! and wire up the entry point:
//!
//! ```ignore
//! #![feature(custom_test_frameworks)]
//! #![test_runner(kaos_kernel::testing::test_runner)]
//! #![reexport_test_harness_main = "test_main"]
//! ```
//!
//! Then mark test functions with `#[test_case]`:
//! ```ignore
//! #[test_case]
//! fn test_simple_assertion() {
//!     assert_eq!(1 + 1, 2);
//! }
//! ```
//!
//! The compiler collects all `#[test_case]` functions and generates a `test_main()`
//! entry point that passes them to [`test_runner`]. Call `test_main()` from your
//! `KernelMain` after performing any required initialization.
//!
//! Run tests with: `cargo test` or `./scripts/run_tests.sh`

use crate::arch::qemu::{exit_qemu, QemuExitCode};
use crate::{debug, debugln};
use core::sync::atomic::{AtomicU32, Ordering};

// ANSI color codes for terminal output via serial
const GREEN: &str = "\x1b[0;32m";
const RED: &str = "\x1b[0;31m";
const CYAN: &str = "\x1b[0;36m";
const RESET: &str = "\x1b[0m";

/// Tracks how many tests have passed so far (used by the panic handler to
/// print an accurate summary even when a test fails).
static TESTS_PASSED: AtomicU32 = AtomicU32::new(0);
static TESTS_TOTAL: AtomicU32 = AtomicU32::new(0);

/// Trait for types that can be run as tests
///
/// This trait allows us to customize how different test types are executed
/// and reported.
pub trait Testable {
    /// Run the test and report results
    fn run(&self);
}

/// Implement Testable for any function with no arguments
impl<T: Fn()> Testable for T {
    fn run(&self) {
        // Print test name (using core::any::type_name to get function name)
        debug!("  {}...", core::any::type_name::<T>());

        // Run the test - if it panics, the panic handler will catch it
        self();

        // If we get here, the test passed
        debugln!(" {}[ok]{}", GREEN, RESET);
        TESTS_PASSED.fetch_add(1, Ordering::Relaxed);
    }
}

/// The main test runner function
///
/// This function is called by the test harness with a slice of all test functions.
/// It runs each test and exits QEMU with the appropriate exit code.
pub fn test_runner(tests: &[&dyn Testable]) {
    TESTS_TOTAL.store(tests.len() as u32, Ordering::Relaxed);
    TESTS_PASSED.store(0, Ordering::Relaxed);

    debugln!("Running {} tests:", tests.len());
    debugln!();

    for test in tests {
        test.run();
    }

    debugln!();
    print_summary();

    exit_qemu(QemuExitCode::Success);
}

/// Called when a test panics
///
/// This function should be called from the panic handler when in test mode.
/// It outputs the failure information and exits QEMU with a failure code.
pub fn test_panic_handler(info: &core::panic::PanicInfo) -> ! {
    debugln!(" {}[FAILED]{}", RED, RESET);
    debugln!();

    if let Some(location) = info.location() {
        debugln!("  Location: {}:{}", location.file(), location.line());
    }

    if let Some(message) = info.message().as_str() {
        debugln!("  Message: {}", message);
    }

    debugln!();
    print_summary();

    exit_qemu(QemuExitCode::Failed);
}

/// Print a colored test result summary
fn print_summary() {
    let total = TESTS_TOTAL.load(Ordering::Relaxed);
    let passed = TESTS_PASSED.load(Ordering::Relaxed);
    let failed = total - passed;

    debugln!("Total:  {}", total);
    debugln!("{}Passed: {}{}", GREEN, passed, RESET);
    if failed > 0 {
        debugln!("{}Failed: {}{}", RED, failed, RESET);
    } else {
        debugln!("Failed: 0");
    }
}

/// Macro to assert equality with better error messages for tests
#[macro_export]
macro_rules! test_assert_eq {
    ($left:expr, $right:expr) => {
        if $left != $right {
            panic!(
                "assertion failed: `(left == right)`\n  left: `{:?}`\n right: `{:?}`",
                $left, $right
            );
        }
    };
    ($left:expr, $right:expr, $($arg:tt)+) => {
        if $left != $right {
            panic!(
                "assertion failed: `(left == right)`\n  left: `{:?}`\n right: `{:?}`\n note: {}",
                $left, $right, format_args!($($arg)+)
            );
        }
    };
}

/// Macro to assert a condition is true
#[macro_export]
macro_rules! test_assert {
    ($cond:expr) => {
        if !$cond {
            panic!("assertion failed: {}", stringify!($cond));
        }
    };
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            panic!("assertion failed: {}\n note: {}", stringify!($cond), format_args!($($arg)+));
        }
    };
}
