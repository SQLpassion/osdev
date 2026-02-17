//! Panic contract test for ATA access before init.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::drivers::ata;

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
    let expected = "ATA driver not initialized";
    let matches_contract = info
        .message()
        .as_str()
        .is_some_and(|m| m.contains(expected));

    if matches_contract {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: read_sectors panics before ata::init.
/// Given: ATA subsystem was not initialized in this test binary.
/// When: read_sectors is called.
/// Then: The call must panic with the documented contract message.
#[test_case]
fn test_read_sectors_panics_before_init() {
    let mut buffer = [0u8; 512];
    let _ = ata::read_sectors(&mut buffer, 0, 1);
}
