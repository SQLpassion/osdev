//! Scheduler error-path tests that require a pristine non-initialized state.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::scheduler::{self as sched, SpawnError};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

extern "C" fn dummy_task() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: spawn fails with not initialized.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "spawn fails with not initialized".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spawn_fails_with_not_initialized() {
    let err = sched::spawn(dummy_task).expect_err("spawn before init must fail");
    assert!(
        matches!(err, SpawnError::NotInitialized),
        "expected NotInitialized when spawning before scheduler init"
    );
}

/// Contract: yield now without scheduler init does not initialize scheduler.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "yield now without scheduler init does not initialize scheduler".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_yield_now_without_scheduler_init_does_not_initialize_scheduler() {
    sched::yield_now();

    assert!(
        !sched::is_running(),
        "yield_now without init must not transition scheduler into running state"
    );

    let err = sched::spawn(dummy_task).expect_err("scheduler should still be uninitialized");
    assert!(
        matches!(err, SpawnError::NotInitialized),
        "yield_now must not initialize scheduler implicitly"
    );
}
