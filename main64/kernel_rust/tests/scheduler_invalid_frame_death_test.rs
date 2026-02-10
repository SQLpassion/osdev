//! Adversarial scheduler-frame integrity test.
//!
//! Verifies that feeding a clearly invalid current-frame pointer into the
//! scheduler does not clobber task context pointers.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts::{self, SavedRegisters};
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

extern "C" fn dummy_task_a() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn dummy_task_b() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: invalid task frame detection never writes outside task stack.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "invalid task frame detection never writes outside task stack".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_invalid_task_frame_detection_never_writes_outside_task_stack() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn(dummy_task_b).expect("task B should spawn");
    let frame_a_before = sched::task_frame_ptr(task_a).expect("task A frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    // Enter task A once so it becomes the current running slot.
    let running = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        running == frame_a_before,
        "first tick should select task A for deterministic setup"
    );

    // Feed an obviously invalid, non-mapped frame pointer.
    let invalid_frame = 0x1usize as *mut SavedRegisters;
    let next = sched::on_timer_tick(invalid_frame);

    let frame_a_after = sched::task_frame_ptr(task_a).expect("task A frame should still exist");
    assert!(
        frame_a_after == frame_a_before,
        "invalid frame pointer must not overwrite saved task frame pointer"
    );
    assert!(
        next == frame_a_before,
        "scheduler should fall back to a known-good saved task frame"
    );
}
