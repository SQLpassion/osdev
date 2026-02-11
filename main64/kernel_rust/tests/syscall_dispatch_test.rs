//! Syscall dispatcher integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::syscall::{self, SyscallId};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: syscall ids remain stable.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "syscall ids remain stable".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_syscall_ids_are_stable() {
    assert!(SyscallId::Yield as u64 == 0, "Yield syscall id changed");
    assert!(SyscallId::WriteSerial as u64 == 1, "WriteSerial syscall id changed");
    assert!(SyscallId::Exit as u64 == 2, "Exit syscall id changed");
}

/// Contract: unknown syscall returns enosys.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "unknown syscall returns enosys".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_unknown_syscall_returns_enosys() {
    let ret = syscall::dispatch(0xDEAD, 1, 2, 3, 4);
    assert!(ret == syscall::ERR_ENOSYS, "unknown syscall must return ENOSYS");
}

/// Contract: write_serial rejects null pointer when len > 0.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "write_serial rejects null pointer when len > 0".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_write_serial_null_ptr_with_len_returns_einval() {
    let ret = syscall::dispatch(SyscallId::WriteSerial as u64, 0, 1, 0, 0);
    assert!(
        ret == syscall::ERR_EINVAL,
        "write_serial with null pointer and len>0 must return EINVAL"
    );
}

/// Contract: write_serial with zero length is a no-op success.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "write_serial with zero length is a no-op success".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_write_serial_zero_len_returns_zero() {
    let ret = syscall::dispatch(SyscallId::WriteSerial as u64, 0, 0, 0, 0);
    assert!(ret == 0, "write_serial len=0 must return 0");
}
