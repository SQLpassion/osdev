//! Death test for fatal page-fault handling path.
//!
//! This test triggers the VMM protection-fault panic path and treats the
//! expected panic as success.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::memory::{heap, pmm, vmm};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Minimal memory stack required by VMM routines.
    pmm::init(false);
    vmm::init(false);
    heap::init(false);

    test_main();

    // The test must panic before reaching this point.
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let expected = "VMM: protection page fault";
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

/// Contract: page fault without mapping exits via test panic handler.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "page fault without mapping exits via test panic handler".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_page_fault_without_mapping_exits_via_test_panic_handler() {
    // Error code bit0 (P=1) marks a protection fault and must not be handled
    // by demand paging.
    vmm::handle_page_fault(0xFFFF_8000_1234_5000, 1);
}
