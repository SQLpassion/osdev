//! Round-robin scheduler integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts::{self, SavedRegisters};
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

extern "C" fn dummy_task_c() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[test_case]
fn test_start_without_tasks_does_not_enter_running_state() {
    sched::init();
    sched::start();
    assert!(
        !sched::is_running(),
        "scheduler must stay stopped when start is called without tasks"
    );
}

#[test_case]
fn test_scheduler_round_robin_pointer_sequence() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first timer tick should switch to task A");

    current = sched::on_timer_tick(current);
    assert!(current == frame_b, "second timer tick should switch to task B");

    current = sched::on_timer_tick(current);
    assert!(current == frame_c, "third timer tick should switch to task C");

    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "fourth timer tick should wrap to task A");
}

#[test_case]
fn test_scheduler_capacity_limit() {
    sched::init();

    for _ in 0..8 {
        sched::spawn(dummy_task_a).expect("spawn within pool capacity should succeed");
    }

    let err = sched::spawn(dummy_task_b).expect_err("spawn beyond capacity must fail");
    assert!(
        matches!(err, SpawnError::CapacityExceeded),
        "expected CapacityExceeded when task pool is full"
    );
}

#[test_case]
fn test_spawn_allocates_distinct_task_frames() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist") as usize;
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist") as usize;
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist") as usize;

    assert!(frame_a != frame_b, "task A and B frames must differ");
    assert!(frame_b != frame_c, "task B and C frames must differ");
    assert!(frame_a != frame_c, "task A and C frames must differ");
}

#[test_case]
fn test_task_frame_iret_defaults_are_kernel_mode() {
    sched::init();
    let task = sched::spawn(dummy_task_a).expect("task should spawn");
    let frame = sched::task_frame_ptr(task).expect("task frame should exist") as usize;

    let iret_ptr = frame + core::mem::size_of::<SavedRegisters>();
    // SAFETY:
    // - `task_frame_ptr` points into scheduler-owned stack memory.
    // - Initial frame layout writes `InterruptStackFrame` directly behind `SavedRegisters`.
    let iret = unsafe {
        &*(iret_ptr as *const kaos_kernel::arch::interrupts::InterruptStackFrame)
    };

    assert!(iret.cs == 0x08, "initial task frame must use kernel CS");
    assert!(
        (iret.rflags & 0x2) != 0,
        "initial task frame must keep reserved RFLAGS bit 1 set"
    );
}

#[test_case]
fn test_scheduler_recovers_when_current_frame_slot_mismatches_expected_slot() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // First tick selects A.
    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first timer tick should switch to task A");

    // Feed an unexpected frame (C) where A was expected; scheduler should recover.
    current = sched::on_timer_tick(frame_c);
    assert!(
        current == frame_a,
        "scheduler should realign to the slot implied by current_frame and continue RR"
    );

    current = sched::on_timer_tick(current);
    assert!(current == frame_b, "round robin should continue with task B");

    current = sched::on_timer_tick(current);
    assert!(current == frame_c, "round robin should continue with task C");
}

#[test_case]
fn test_scheduler_mismatch_fallback_reselects_valid_task_frame() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn(dummy_task_b).expect("task B should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    // First tick enters task A.
    let _ = sched::on_timer_tick(bootstrap_ptr);

    // Feed a non-task frame; scheduler must fall back to a known-good task frame.
    let next = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        next == frame_a,
        "fallback path should reselect a valid runnable task frame"
    );
}

#[test_case]
fn test_unmapped_current_frame_does_not_clobber_saved_task_context() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn(dummy_task_b).expect("task B should spawn");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    // First tick switches into task A.
    let current = sched::on_timer_tick(bootstrap_ptr);
    let saved_a_before = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    assert!(current == saved_a_before, "task A should be selected first");

    // Feed an unmapped/non-task frame: scheduler must not overwrite A with bootstrap frame.
    let _ = sched::on_timer_tick(bootstrap_ptr);
    let saved_a_after = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    assert!(
        saved_a_after == saved_a_before,
        "unexpected current frame must not overwrite saved task context"
    );
}

#[test_case]
fn test_request_stop_returns_to_bootstrap_frame_and_stops_scheduler() {
    sched::init();

    let task_a = sched::spawn(dummy_task_a).expect("task A should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    let running = sched::on_timer_tick(bootstrap_ptr);
    assert!(running == frame_a, "first timer tick should enter task A");
    assert!(sched::is_running(), "scheduler should report running after start");

    sched::request_stop();
    let after_stop = sched::on_timer_tick(running);
    assert!(
        after_stop == bootstrap_ptr,
        "stop request must switch back to the original bootstrap frame"
    );
    assert!(
        !sched::is_running(),
        "scheduler must report stopped after stop request"
    );

    let new_task = sched::spawn(dummy_task_b).expect("spawn should work again after stop");
    let new_frame = sched::task_frame_ptr(new_task).expect("new task frame should exist");
    sched::start();
    let resumed = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        resumed == new_frame,
        "scheduler should be able to start again after a stop cycle"
    );
}
