//! Panic contract test to ensure the fatal exception path does not deadlock
//! if a page fault happens while the serial lock is held.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::fmt;
use core::panic::PanicInfo;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::debugln;

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
    let expected = "VMM: protection page fault";
    let matches_contract = kaos_kernel::testing::panic_message_contains(info, expected);

    if matches_contract {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

struct FaultingFormatter;

impl fmt::Display for FaultingFormatter {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Trigger a #PF by reading from a non-present address.
        // We use address 0x0 which is not mapped in the kernel.
        unsafe {
            let _ = core::ptr::read_volatile(0x0 as *const u8);
        }
        Ok(())
    }
}

/// Contract: A kernel page fault while the serial lock is held does not deadlock.
/// Given: The serial lock is acquired by `debugln!`.
/// When: A format parameter triggers a #PF.
/// Then: The exception handler correctly panics and avoids the deadlock.
#[test_case]
fn test_serial_deadlock_on_exception() {
    debugln!("Provoking fault: {}", FaultingFormatter);
}
