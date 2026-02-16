//! Application task launch integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::apps::{self, RunAppError};
use kaos_kernel::arch::interrupts;
use kaos_kernel::scheduler as sched;

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

/// Contract: known apps spawn as dedicated scheduler tasks.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "known apps spawn as dedicated scheduler tasks".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_known_apps_spawn_as_dedicated_scheduler_tasks() {
    sched::init();

    let hello_task = apps::spawn_app("hello").expect("hello app should spawn");
    let counter_task = apps::spawn_app("counter").expect("counter app should spawn");

    assert_ne!(
        hello_task, counter_task,
        "hello and counter must run in separate task slots"
    );
    assert!(
        sched::task_frame_ptr(hello_task).is_some(),
        "hello task frame must exist after spawn"
    );
    assert!(
        sched::task_frame_ptr(counter_task).is_some(),
        "counter task frame must exist after spawn"
    );
}

/// Contract: unknown app names are rejected with UnknownApp.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "unknown app names are rejected with UnknownApp".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spawn_unknown_app_returns_unknown_error() {
    sched::init();

    let err = apps::spawn_app("does-not-exist").expect_err("unknown app must fail");
    assert!(
        matches!(err, RunAppError::UnknownApp),
        "unknown app must return RunAppError::UnknownApp"
    );
}

/// Contract: spawned app tasks can be terminated and removed from scheduler slots.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "spawned app tasks can be terminated and removed from scheduler slots".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spawned_app_task_can_be_terminated_and_slot_freed() {
    sched::init();

    let task_id = apps::spawn_app("hello").expect("hello app should spawn");
    assert!(
        sched::task_frame_ptr(task_id).is_some(),
        "task frame must exist"
    );

    let removed = sched::terminate_task(task_id);
    assert!(removed, "terminate_task should remove spawned app task");
    assert!(
        sched::task_frame_ptr(task_id).is_none(),
        "task frame must be gone after termination"
    );
}
