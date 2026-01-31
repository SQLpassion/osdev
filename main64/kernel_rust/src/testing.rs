//! Test Framework for KAOS Kernel
//!
//! This module provides a custom test framework for running tests in a bare-metal
//! environment. Tests are run inside QEMU and results are output via serial port.
//!
//! # Usage
//!
//! Mark test functions with `#[test_case]`:
//! ```
//! #[test_case]
//! fn test_simple_assertion() {
//!     assert_eq!(1 + 1, 2);
//! }
//! ```
//!
//! Run tests with: `cargo test`
//! This will build a test binary, run it in QEMU, and check the exit code.

use crate::arch::qemu::{exit_qemu, QemuExitCode};
use crate::{debug, debugln};

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
        debugln!(" [ok]");
    }
}

/// The main test runner function
///
/// This function is called by the test harness with a slice of all test functions.
/// It runs each test and exits QEMU with the appropriate exit code.
pub fn test_runner(tests: &[&dyn Testable]) {
    debugln!("========================================");
    debugln!("    KAOS Kernel Test Framework");
    debugln!("========================================");
    debugln!();
    debugln!("Running {} tests:", tests.len());
    debugln!();

    for test in tests {
        test.run();
    }

    debugln!();
    debugln!("========================================");
    debugln!("All {} tests passed!", tests.len());
    debugln!("========================================");

    exit_qemu(QemuExitCode::Success);
}

/// Called when a test panics
///
/// This function should be called from the panic handler when in test mode.
/// It outputs the failure information and exits QEMU with a failure code.
pub fn test_panic_handler(info: &core::panic::PanicInfo) -> ! {
    debugln!(" [FAILED]");
    debugln!();
    debugln!("========================================");
    debugln!("TEST FAILED!");
    debugln!("========================================");

    if let Some(location) = info.location() {
        debugln!("Location: {}:{}", location.file(), location.line());
    }

    if let Some(message) = info.message().as_str() {
        debugln!("Message: {}", message);
    }

    debugln!();

    exit_qemu(QemuExitCode::Failed);
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
