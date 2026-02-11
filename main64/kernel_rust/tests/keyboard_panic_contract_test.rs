//! Panic contract test for keyboard blocking read.
//!
//! This binary verifies that calling `read_char_blocking` outside a scheduled
//! task context panics with the documented contract message.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::drivers::keyboard;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();

    // If we ever reach this point, the expected panic did not happen.
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let expected = "read_char_blocking called outside scheduled task";
    let matches_contract = info.message().as_str().is_some_and(|m| m.contains(expected));

    if matches_contract {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: read char blocking panics outside scheduled task.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "read char blocking panics outside scheduled task".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_read_char_blocking_panics_outside_scheduled_task() {
    keyboard::init();
    let _ = keyboard::read_char_blocking();
}
