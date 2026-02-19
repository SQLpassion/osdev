//! Round-robin scheduler integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::gdt;
use kaos_kernel::arch::interrupts::{self, SavedRegisters};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, TaskState};
use kaos_kernel::sync::singlewaitqueue::SingleWaitQueue;
use kaos_kernel::sync::waitqueue::WaitQueue;
use kaos_kernel::sync::waitqueue_adapter;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);

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

extern "C" fn dummy_task_d() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn dummy_task_e() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: start without tasks does not enter running state.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "start without tasks does not enter running state".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_start_without_tasks_does_not_enter_running_state() {
    sched::init();
    sched::start();
    assert!(
        !sched::is_running(),
        "scheduler must stay stopped when start is called without tasks"
    );
}

/// Contract: scheduler api preserves enabled interrupt state.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler api preserves enabled interrupt state".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_api_preserves_enabled_interrupt_state() {
    // Reset scheduler state first so no previous test can leave it running
    // while we intentionally enable hardware interrupts below.
    interrupts::disable();
    sched::init();

    interrupts::enable();
    assert!(
        interrupts::are_enabled(),
        "interrupts should be enabled at test start"
    );

    sched::init();
    let _ = sched::spawn_kernel_task(dummy_task_a).expect("spawn should succeed after init");
    // Do not call `start()` here: with IRQ0 unmasked in the test environment,
    // a hardware timer tick could immediately switch into `dummy_task_a` and
    // never return to this test function.

    assert!(
        interrupts::are_enabled(),
        "scheduler API calls must restore enabled interrupt state"
    );

    interrupts::disable();
}

/// Contract: scheduler round robin pointer sequence.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler round robin pointer sequence".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_round_robin_pointer_sequence() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_a,
        "first timer tick should switch to task A"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "second timer tick should switch to task B"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_c,
        "third timer tick should switch to task C"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_a,
        "fourth timer tick should wrap to task A"
    );
}

/// Contract: scheduler round robin pointer sequence with five tasks.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler round robin pointer sequence with five tasks".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_round_robin_pointer_sequence_with_five_tasks() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");
    let task_d = sched::spawn_kernel_task(dummy_task_d).expect("task D should spawn");
    let task_e = sched::spawn_kernel_task(dummy_task_e).expect("task E should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");
    let frame_d = sched::task_frame_ptr(task_d).expect("task D frame should exist");
    let frame_e = sched::task_frame_ptr(task_e).expect("task E frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_a,
        "first timer tick should switch to task A"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "second timer tick should switch to task B"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_c,
        "third timer tick should switch to task C"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_d,
        "fourth timer tick should switch to task D"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_e,
        "fifth timer tick should switch to task E"
    );

    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "sixth timer tick should wrap to task A");
}

/// Contract: scheduler sets running state on selected task and restores previous task to ready.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler sets running state on selected task and restores previous task to ready".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_marks_selected_task_running_and_previous_ready() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // Tick 1: select task A.
    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should select task A");
    assert!(
        sched::task_state(task_a) == Some(TaskState::Running),
        "selected task A must be marked Running"
    );
    assert!(
        sched::task_state(task_b) == Some(TaskState::Ready),
        "non-selected task B must remain Ready"
    );

    // Tick 2: select task B and demote task A back to Ready.
    current = sched::on_timer_tick(current);
    assert!(current == frame_b, "second tick should select task B");
    assert!(
        sched::task_state(task_b) == Some(TaskState::Running),
        "selected task B must be marked Running"
    );
    assert!(
        sched::task_state(task_a) == Some(TaskState::Ready),
        "previously running task A must be restored to Ready"
    );
}

/// Contract: selecting user task updates tss rsp0 from task context.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "selecting user task updates tss rsp0 from task context".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_selecting_user_task_updates_tss_rsp0_from_task_context() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let expected_rsp0 = 0xFFFF_8000_0013_7000u64;

    assert!(
        sched::set_task_user_context(task_a, 0x0010_0000, 0x0000_7FFF_FFFF_F000, expected_rsp0),
        "setting user context for existing task must succeed"
    );
    assert!(
        sched::is_user_task(task_a),
        "task A must be flagged as user task"
    );
    assert!(
        sched::task_context(task_a) == Some((0x0010_0000, 0x0000_7FFF_FFFF_F000, expected_rsp0)),
        "stored task context must match the configured values"
    );

    // Seed a different value so the test proves an update happened.
    gdt::set_kernel_rsp0(0xFFFF_8000_0000_1000);

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let current = sched::on_timer_tick(&mut bootstrap as *mut SavedRegisters);
    assert!(current == frame_a, "first tick should select task A");

    assert!(
        gdt::kernel_rsp0() == expected_rsp0,
        "selecting a user task must update TSS.RSP0 to the task's kernel stack top"
    );
}

/// Contract: block and unblock influence next round-robin selections.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "block and unblock influence next round-robin selections".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_block_and_unblock_influence_next_round_robin_selections() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    let mut current = sched::on_timer_tick(bootstrap_ptr);
    assert!(current == frame_a, "first tick should select task A");

    sched::block_task(task_b);
    current = sched::on_timer_tick(current);
    assert!(
        current == frame_a,
        "with task B blocked, scheduler should keep selecting task A"
    );

    sched::unblock_task(task_b);
    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "after unblocking task B, scheduler should select task B next"
    );
}

/// Contract: spawning tasks after scheduler start integrates them into round robin.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "spawning tasks after scheduler start integrates them into round robin".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spawning_tasks_after_scheduler_start_integrates_into_round_robin() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_a,
        "first tick should select initial task A"
    );

    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn after start");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn after start");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "newly spawned task B should be scheduled next"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_c,
        "newly spawned task C should be scheduled next"
    );

    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "round robin should wrap back to task A");
}

/// Contract: terminating running task must not overwrite bootstrap frame with stale task frame.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "terminating running task must not overwrite bootstrap frame with stale task frame".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_terminated_task_frame_does_not_replace_bootstrap_frame() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    // Capture a valid bootstrap frame first, then run task A.
    let frame_a = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        frame_a == sched::task_frame_ptr(task_a).expect("task A frame should exist"),
        "first tick should select task A"
    );

    // Remove currently running task A, leaving `frame_a` as a stale pointer.
    let removed = sched::terminate_task(task_a);
    assert!(removed, "task A must terminate");

    // Next tick with stale frame must schedule task B (not treat stale frame as bootstrap).
    let current = sched::on_timer_tick(frame_a);
    assert!(current == frame_b, "scheduler should continue with task B");

    // If task B blocks, scheduler must return original bootstrap frame.
    sched::block_task(task_b);
    let next = sched::on_timer_tick(current);
    assert!(
        next == bootstrap_ptr,
        "all-blocked path must return original bootstrap frame"
    );
}

/// Contract: terminate task removes slot from round robin and allows reuse.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "terminate task removes slot from round robin and allows reuse".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_terminate_task_removes_slot_from_round_robin_and_allows_reuse() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should select task A");

    assert!(
        sched::terminate_task(task_b),
        "terminate_task should remove an existing task"
    );
    assert!(
        sched::task_frame_ptr(task_b).is_none(),
        "terminated task B should no longer expose a frame pointer"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_c,
        "after removing task B, scheduler should continue with task C"
    );

    let task_d = sched::spawn_kernel_task(dummy_task_d)
        .expect("task D should spawn into freed slot capacity");
    let frame_d = sched::task_frame_ptr(task_d).expect("task D frame should exist");

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_d,
        "newly spawned task D should participate in round robin after task C"
    );
}

/// Contract: terminating running task switches to the next runnable task.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "terminating running task switches to the next runnable task".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_terminating_running_task_switches_to_next_runnable_task() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should enter task A");

    assert!(
        sched::terminate_task(task_a),
        "terminate_task should remove the currently running task"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "scheduler should switch to remaining runnable task B after task A termination"
    );
}

/// Contract: terminate task reports false for non-existent or stale task id.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "terminate task reports false for non-existent or stale task id".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_terminate_task_reports_false_for_non_existent_or_stale_task_id() {
    sched::init();

    assert!(
        !sched::terminate_task(0),
        "terminate_task must return false for an unused slot"
    );

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    assert!(
        sched::terminate_task(task_a),
        "first terminate should remove existing task A"
    );
    assert!(
        !sched::terminate_task(task_a),
        "second terminate should report false because task A is already removed"
    );
}

/// Contract: terminating multiple tasks allows same-count respawn cycle.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "terminating multiple tasks allows same-count respawn cycle".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_terminating_multiple_tasks_allows_same_count_respawn_cycle() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");

    assert!(sched::terminate_task(task_a), "task A should terminate");
    assert!(sched::terminate_task(task_b), "task B should terminate");
    assert!(sched::terminate_task(task_c), "task C should terminate");

    assert!(
        sched::task_frame_ptr(task_a).is_none()
            && sched::task_frame_ptr(task_b).is_none()
            && sched::task_frame_ptr(task_c).is_none(),
        "all terminated tasks must be fully removed from scheduler slots"
    );

    let respawn_a = sched::spawn_kernel_task(dummy_task_a).expect("respawn A should succeed");
    let respawn_b = sched::spawn_kernel_task(dummy_task_b).expect("respawn B should succeed");
    let respawn_c = sched::spawn_kernel_task(dummy_task_c).expect("respawn C should succeed");

    assert!(
        sched::task_frame_ptr(respawn_a).is_some()
            && sched::task_frame_ptr(respawn_b).is_some()
            && sched::task_frame_ptr(respawn_c).is_some(),
        "after bulk terminate, scheduler must accept same-count respawn"
    );
}

/// Contract: scheduler grows dynamically beyond former 8-task limit.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler grows dynamically beyond former 8-task limit".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_dynamic_capacity() {
    sched::init();

    // Spawn well beyond the former MAX_TASKS = 8 hard limit.
    // With heap-backed Vecs the scheduler must accept all of these.
    for i in 0..16 {
        sched::spawn_kernel_task(dummy_task_a)
            .unwrap_or_else(|e| panic!("spawn #{i} failed: {e:?}"));
    }

    // The 17th spawn must also succeed — capacity is only bounded by the heap.
    sched::spawn_kernel_task(dummy_task_b).expect("spawn beyond former static limit must succeed");
}

/// Contract: spawn allocates distinct task frames.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "spawn allocates distinct task frames".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spawn_allocates_distinct_task_frames() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist") as usize;
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist") as usize;
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist") as usize;

    assert!(frame_a != frame_b, "task A and B frames must differ");
    assert!(frame_b != frame_c, "task B and C frames must differ");
    assert!(frame_a != frame_c, "task A and C frames must differ");
}

/// Contract: task frame iret defaults are kernel mode.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "task frame iret defaults are kernel mode".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_task_frame_iret_defaults_are_kernel_mode() {
    sched::init();
    let task = sched::spawn_kernel_task(dummy_task_a).expect("task should spawn");
    let frame = sched::task_frame_ptr(task).expect("task frame should exist") as usize;

    let iret_ptr = frame + core::mem::size_of::<SavedRegisters>();
    // SAFETY:
    // - `task_frame_ptr` points into scheduler-owned stack memory.
    // - Initial frame layout writes `InterruptStackFrame` directly behind `SavedRegisters`.
    let iret = unsafe { &*(iret_ptr as *const kaos_kernel::arch::interrupts::InterruptStackFrame) };

    assert!(iret.cs == 0x08, "initial task frame must use kernel CS");
    assert!(
        (iret.rflags & 0x2) != 0,
        "initial task frame must keep reserved RFLAGS bit 1 set"
    );
}

/// Contract: spawn user builds ring3 iret frame with configured selectors and pointers.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "spawn user builds ring3 iret frame with configured selectors and pointers".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spawn_user_builds_ring3_iret_frame_with_configured_selectors_and_pointers() {
    sched::init();
    let user_entry = 0x0000_7000_0000_1000u64;
    let user_rsp = 0x0000_7FFF_EFFF_F000u64;
    let user_cr3 = 0x0000_0000_0040_0000u64;

    let task_id = sched::spawn_user_task(user_entry, user_rsp, user_cr3)
        .expect("user task spawn should succeed");

    assert!(
        sched::is_user_task(task_id),
        "spawned task must be marked as user task"
    );
    let (stored_cr3, stored_user_rsp, _stored_kernel_rsp_top) =
        sched::task_context(task_id).expect("user context tuple must exist for spawned user task");
    assert!(
        stored_cr3 == user_cr3 && stored_user_rsp == user_rsp,
        "user context should store configured CR3 and user RSP"
    );

    let iret = sched::task_iret_frame(task_id).expect("user task iret frame must exist");
    assert!(
        iret.rip == user_entry,
        "user iret RIP must match configured entry"
    );
    assert!(
        iret.rsp == user_rsp,
        "user iret RSP must match configured user stack"
    );
    assert!(
        iret.cs == kaos_kernel::arch::gdt::USER_CODE_SELECTOR as u64,
        "user iret CS must use ring-3 code selector"
    );
    assert!(
        iret.ss == kaos_kernel::arch::gdt::USER_DATA_SELECTOR as u64,
        "user iret SS must use ring-3 data selector"
    );
    assert!(
        (iret.rflags & (1 << 9)) != 0,
        "user iret RFLAGS must have IF set for timer preemption"
    );
}

/// Contract: scheduler switches cr3 between kernel and user tasks when enabled.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler switches cr3 between kernel and user tasks when enabled".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_switches_cr3_between_kernel_and_user_tasks_when_enabled() {
    sched::init();
    let kernel_cr3 = vmm::get_pml4_address();
    sched::set_kernel_address_space_cr3(kernel_cr3);

    let kernel_task =
        sched::spawn_kernel_task(dummy_task_a).expect("kernel task spawn should succeed");
    let kernel_frame = sched::task_frame_ptr(kernel_task).expect("kernel task frame should exist");

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let user_task = sched::spawn_user_task(vmm::USER_CODE_BASE, vmm::USER_STACK_TOP - 16, user_cr3)
        .expect("user task spawn should succeed");
    let user_frame = sched::task_frame_ptr(user_task).expect("user task frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(
        current == kernel_frame,
        "first tick should pick kernel task"
    );
    assert!(
        vmm::get_pml4_address() == kernel_cr3,
        "kernel task selection should keep kernel CR3 active"
    );

    current = sched::on_timer_tick(current);
    assert!(current == user_frame, "second tick should pick user task");
    assert!(
        vmm::get_pml4_address() == user_cr3,
        "user task selection should switch active CR3 to user CR3"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == kernel_frame,
        "third tick should wrap back to kernel task"
    );
    assert!(
        vmm::get_pml4_address() == kernel_cr3,
        "switching back to kernel task should restore kernel CR3"
    );

    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(user_cr3 / 4096)));
}

/// Contract: reaping user task destroys its address space.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "reaping user task destroys its address space".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_reaping_user_task_destroys_its_address_space() {
    sched::init();
    let kernel_cr3 = vmm::get_pml4_address();
    sched::set_kernel_address_space_cr3(kernel_cr3);

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let user_task = sched::spawn_user_task(vmm::USER_CODE_BASE, vmm::USER_STACK_TOP - 16, user_cr3)
        .expect("user task spawn should succeed");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    current = sched::on_timer_tick(current);
    assert!(
        vmm::get_pml4_address() == user_cr3,
        "user task selection should activate its CR3"
    );

    sched::mark_current_as_zombie();
    let _ = sched::on_timer_tick(current);

    assert!(
        sched::task_state(user_task).is_none(),
        "zombie user task should be reaped on next tick"
    );
    assert!(
        vmm::get_pml4_address() == kernel_cr3,
        "reap path should switch back to kernel CR3 before destroying owned CR3"
    );

    pmm::with_pmm(|mgr| {
        assert!(
            !mgr.release_pfn(user_cr3 / pmm::PAGE_SIZE),
            "user CR3 root must already be released by scheduler reap"
        )
    });
}

/// Contract: scheduler recovers when current frame slot mismatches expected slot.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler recovers when current frame slot mismatches expected slot".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_recovers_when_current_frame_slot_mismatches_expected_slot() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");

    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // First tick selects A.
    current = sched::on_timer_tick(current);
    assert!(
        current == frame_a,
        "first timer tick should switch to task A"
    );

    // Feed an unexpected frame (C) where A was expected; scheduler should recover.
    current = sched::on_timer_tick(frame_c);
    assert!(
        current == frame_a,
        "scheduler should realign to the slot implied by current_frame and continue RR"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "round robin should continue with task B"
    );

    current = sched::on_timer_tick(current);
    assert!(
        current == frame_c,
        "round robin should continue with task C"
    );
}

/// Contract: scheduler mismatch fallback reselects valid task frame.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler mismatch fallback reselects valid task frame".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_mismatch_fallback_reselects_valid_task_frame() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");

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

/// Contract: unmapped current frame does not clobber saved task context.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "unmapped current frame does not clobber saved task context".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_unmapped_current_frame_does_not_clobber_saved_task_context() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");

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

/// Contract: invalid task frame detection never writes outside task stack.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "invalid task frame detection never writes outside task stack".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_invalid_task_frame_detection_never_writes_outside_task_stack() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
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
    let invalid_frame = core::ptr::dangling_mut::<SavedRegisters>();
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

/// Contract: request stop returns to bootstrap frame and stops scheduler.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "request stop returns to bootstrap frame and stops scheduler".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_request_stop_returns_to_bootstrap_frame_and_stops_scheduler() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    let running = sched::on_timer_tick(bootstrap_ptr);
    assert!(running == frame_a, "first timer tick should enter task A");
    assert!(
        sched::is_running(),
        "scheduler should report running after start"
    );

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

    let new_task =
        sched::spawn_kernel_task(dummy_task_b).expect("spawn should work again after stop");
    let new_frame = sched::task_frame_ptr(new_task).expect("new task frame should exist");
    sched::start();
    let resumed = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        resumed == new_frame,
        "scheduler should be able to start again after a stop cycle"
    );

    // Cleanup to keep later tests isolated: stop scheduler again.
    sched::request_stop();
    let stopped_again = sched::on_timer_tick(resumed);
    assert!(
        stopped_again == bootstrap_ptr,
        "cleanup stop must return to bootstrap frame"
    );
    assert!(
        !sched::is_running(),
        "scheduler must be stopped at end of test"
    );
}

/// Contract: scheduler reinit clears blocked state from previous run.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler reinit clears blocked state from previous run".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_reinit_clears_blocked_state_from_previous_run() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::block_task(task_a);
    sched::start();

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    let first = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        first == frame_b,
        "blocked task A must not be selected in first scheduler run"
    );
    assert!(first != frame_a, "sanity check: blocked frame must differ");

    sched::request_stop();
    let _ = sched::on_timer_tick(first);
    assert!(
        !sched::is_running(),
        "scheduler should be stopped after stop request"
    );

    sched::init();
    let new_task =
        sched::spawn_kernel_task(dummy_task_a).expect("task spawn after re-init must work");
    let new_frame = sched::task_frame_ptr(new_task).expect("new frame should exist");
    sched::start();

    let resumed = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        resumed == new_frame,
        "re-init must start from clean state without stale blocked tasks"
    );
}

/// Contract: waitqueue wake all returns all registered waiters once.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "waitqueue wake all returns all registered waiters once".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_waitqueue_wake_all_returns_all_registered_waiters_once() {
    let q: WaitQueue<8> = WaitQueue::new();

    assert!(q.register_waiter(1), "register waiter 1 must succeed");
    assert!(q.register_waiter(3), "register waiter 3 must succeed");
    assert!(q.register_waiter(6), "register waiter 6 must succeed");

    let mut woke = [usize::MAX; 8];
    let mut count = 0usize;
    q.wake_all(|task_id| {
        woke[count] = task_id;
        count += 1;
    });

    assert!(count == 3, "wake_all should wake exactly 3 waiters");
    assert!(woke[0] == 1, "first woken waiter should be slot 1");
    assert!(woke[1] == 3, "second woken waiter should be slot 3");
    assert!(woke[2] == 6, "third woken waiter should be slot 6");

    let mut woke_again = false;
    q.wake_all(|_| woke_again = true);
    assert!(
        !woke_again,
        "wake_all should clear waiter flags so second wake has no targets"
    );
}

/// Contract: single waitqueue wake all wakes one and clears slot.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "single waitqueue wake all wakes one and clears slot".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_single_waitqueue_wake_all_wakes_one_and_clears_slot() {
    let q = SingleWaitQueue::new();
    assert!(
        q.register_waiter(2),
        "single waiter registration must succeed"
    );

    let mut woke = usize::MAX;
    q.wake_all(|task_id| woke = task_id);
    assert!(
        woke == 2,
        "single wake_all should wake the registered waiter"
    );

    let mut woke_again = false;
    q.wake_all(|_| woke_again = true);
    assert!(
        !woke_again,
        "single wake_all should clear waiter slot after first wake"
    );
}

/// Contract: WaitQueue registration fails only when all slots are occupied.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "WaitQueue registration fails only when all slots are occupied".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_waitqueue_register_fails_when_all_slots_full() {
    let q: WaitQueue<4> = WaitQueue::new();

    // Large task_ids (formerly rejected by the index-based design) must succeed.
    assert!(q.register_waiter(100), "slot 0: large task_id must be accepted");
    assert!(q.register_waiter(200), "slot 1");
    assert!(q.register_waiter(300), "slot 2");
    assert!(q.register_waiter(400), "slot 3");

    // All 4 slots occupied — any further registration must fail.
    assert!(
        !q.register_waiter(500),
        "queue full – must reject when all slots occupied"
    );

    // Re-registering an already-registered task_id is idempotent.
    assert!(
        q.register_waiter(100),
        "re-registration of existing id must succeed"
    );
}

/// Contract: single waitqueue register second waiter is rejected.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "single waitqueue register second waiter is rejected".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_single_waitqueue_register_second_waiter_is_rejected() {
    let q = SingleWaitQueue::new();

    assert!(q.register_waiter(1), "first waiter must register");
    assert!(
        !q.register_waiter(2),
        "different second waiter must be rejected while slot is occupied"
    );
    assert!(
        q.register_waiter(1),
        "same waiter id may re-register while still owner"
    );

    let mut woke = usize::MAX;
    q.wake_all(|task_id| woke = task_id);
    assert!(
        woke == 1,
        "wake_all must wake the originally registered waiter"
    );
}

// ── Zombie lifecycle tests ──────────────────────────────────────────

/// Contract: mark_current_as_zombie transitions running task to Zombie state.
/// Given: A scheduler with two tasks, task A currently running.
/// When: mark_current_as_zombie is called while task A executes.
/// Then: task A's state becomes Zombie; it is skipped in subsequent scheduling.
#[test_case]
fn test_mark_current_as_zombie_sets_zombie_state() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // Enter task A.
    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should select task A");

    // Mark task A as zombie (simulates what syscall Exit does).
    sched::mark_current_as_zombie();
    assert!(
        sched::task_state(task_a) == Some(TaskState::Zombie),
        "mark_current_as_zombie must set task state to Zombie"
    );

    // Next tick: zombie A is skipped, task B is selected.
    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "zombie task must be skipped in round-robin selection"
    );
}

/// Contract: reap_zombies reclaims zombie slots on the next scheduler tick.
/// Given: A scheduler where task A has been marked as zombie.
/// When: Two scheduler ticks have passed (one to leave the zombie's stack,
///       one to trigger reaping).
/// Then: The zombie's slot is freed and can be reused by spawn.
#[test_case]
fn test_reap_zombies_frees_slot_for_reuse() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // Enter task A, then mark it as zombie.
    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should select task A");
    sched::mark_current_as_zombie();

    // Next tick: scheduler leaves task A's stack → reap_zombies runs,
    // freeing the slot.  Task B is selected.
    current = sched::on_timer_tick(current);
    assert!(current == frame_b, "second tick should select task B");

    // Slot A has been reaped — task_state returns None for freed slots.
    assert!(
        sched::task_state(task_a).is_none(),
        "zombie slot must be reaped and freed after scheduler tick"
    );

    // The freed slot can be reused by a new spawn.
    let task_c =
        sched::spawn_kernel_task(dummy_task_c).expect("spawn into reaped slot should succeed");
    let frame_c = sched::task_frame_ptr(task_c).expect("task C frame should exist");

    // Task C participates in the next round-robin cycle.
    current = sched::on_timer_tick(current);
    assert!(
        current == frame_c,
        "newly spawned task in reaped slot should be scheduled"
    );
}

/// Contract: zombie task is never selected even when it is the only task.
/// Given: A single-task scheduler where that task is marked as zombie.
/// When: on_timer_tick is called.
/// Then: The scheduler returns the bootstrap frame (no runnable tasks).
#[test_case]
fn test_zombie_only_task_returns_bootstrap_frame() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    // Enter task A, then mark it as zombie.
    let current = sched::on_timer_tick(bootstrap_ptr);
    assert!(current == frame_a, "first tick should select task A");
    sched::mark_current_as_zombie();

    // Next tick: zombie is the only task → scheduler returns bootstrap.
    let next = sched::on_timer_tick(current);
    assert!(
        next == bootstrap_ptr,
        "with only zombie tasks, scheduler must return bootstrap frame"
    );
}

/// Contract: successive zombies are each reaped on the following tick.
/// Given: Three tasks where A and then another task are marked as zombie
///        on successive ticks.
/// When: on_timer_tick is called after each zombie mark.
/// Then: Each zombie slot is freed and the remaining task continues.
#[test_case]
fn test_successive_zombies_are_reaped_across_ticks() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let task_c = sched::spawn_kernel_task(dummy_task_c).expect("task C should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // Tick 1: select task A, then mark it as zombie.
    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should select task A");
    sched::mark_current_as_zombie();

    // Tick 2: reap zombie A, select the next runnable task,
    // then mark that one as zombie too.
    current = sched::on_timer_tick(current);
    assert!(
        sched::task_state(task_a).is_none(),
        "zombie task A must be reaped after first post-zombie tick"
    );
    // Whatever task was selected, mark it zombie.
    sched::mark_current_as_zombie();

    // Tick 3: the second zombie is reaped, only one task remains.
    let _current = sched::on_timer_tick(current);

    // Exactly one of B or C should still be alive.
    let b_alive = sched::task_state(task_b).is_some();
    let c_alive = sched::task_state(task_c).is_some();
    assert!(
        (b_alive && !c_alive) || (!b_alive && c_alive),
        "exactly one of task B or C must remain after two zombies are reaped"
    );

    // The surviving task must be Running because it was selected on Tick 3.
    let survivor_state = if b_alive {
        sched::task_state(task_b)
    } else {
        sched::task_state(task_c)
    };
    assert!(
        survivor_state == Some(TaskState::Running),
        "the surviving task must be in Running state"
    );
}

/// Contract: waitqueue adapter blocks then wakes task.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "waitqueue adapter blocks then wakes task".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_waitqueue_adapter_blocks_then_wakes_task() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let _task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");

    let q: WaitQueue<8> = WaitQueue::new();
    let blocked = waitqueue_adapter::sleep_if_multi(&q, task_a, || true);
    assert!(
        blocked,
        "sleep_if_multi should block when predicate is true"
    );

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    let first = sched::on_timer_tick(bootstrap_ptr);
    assert!(
        first != frame_a,
        "blocked task A must not be selected while waitqueue sleep is active"
    );

    waitqueue_adapter::wake_all_multi(&q);
    let second = sched::on_timer_tick(first);
    assert!(
        second == frame_a,
        "waking waitqueue must make blocked task A runnable again"
    );
}

/// Contract: foreground wait helper keeps yielding while task is reported alive.
/// Given: A deterministic liveness source and a deterministic yield hook.
/// When: `wait_for_task_exit_with` is executed with those hooks.
/// Then: It must poll the configured task id until liveness flips to false and
///       invoke the yield hook once per alive iteration.
/// Failure Impact: Indicates a regression in foreground-exec waiting semantics.
#[test_case]
fn test_wait_for_task_exit_with_polls_until_liveness_turns_false() {
    let expected_task_id = 7usize;
    let mut remaining_alive_polls = 3usize;
    let mut yield_calls = 0usize;

    sched::wait_for_task_exit_with(
        expected_task_id,
        |observed_task_id| {
            assert!(
                observed_task_id == expected_task_id,
                "wait helper must poll the originally requested task id"
            );

            if remaining_alive_polls > 0 {
                remaining_alive_polls -= 1;
                true
            } else {
                false
            }
        },
        || {
            yield_calls += 1;
        },
    );

    assert!(
        remaining_alive_polls == 0,
        "wait helper must continue polling until liveness is reported as false"
    );
    assert!(
        yield_calls == 3,
        "wait helper must yield once per alive iteration"
    );
}

/// Contract: foreground wait returns immediately for absent tasks.
/// Given: A liveness hook that reports "already absent" on first poll.
/// When: `wait_for_task_exit_with` is called for that task id.
/// Then: It must return without requiring cooperative yields.
/// Failure Impact: Indicates a regression in no-op foreground wait semantics.
#[test_case]
fn test_wait_for_task_exit_returns_immediately_for_absent_task() {
    let mut yield_calls = 0usize;
    sched::wait_for_task_exit_with(usize::MAX, |_task_id| false, || {
        yield_calls += 1;
    });

    assert!(
        yield_calls == 0,
        "wait helper must not yield when task is already absent"
    );
}

// ── Exit syscall integration test ───────────────────────────────────

/// Contract: Exit syscall marks running task as zombie and returns SYSCALL_OK.
/// Given: A scheduler with a running task.
/// When: syscall::dispatch is called with SyscallId::Exit.
/// Then: The return value is SYSCALL_OK and the task transitions to Zombie,
///       which prevents it from being selected in subsequent scheduler ticks.
#[test_case]
fn test_exit_syscall_marks_zombie_and_returns_ok() {
    sched::init();

    let task_a = sched::spawn_kernel_task(dummy_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(dummy_task_b).expect("task B should spawn");
    let frame_a = sched::task_frame_ptr(task_a).expect("task A frame should exist");
    let frame_b = sched::task_frame_ptr(task_b).expect("task B frame should exist");

    sched::start();
    let mut bootstrap = SavedRegisters::default();
    let mut current = &mut bootstrap as *mut SavedRegisters;

    // Enter task A so it becomes the running task.
    current = sched::on_timer_tick(current);
    assert!(current == frame_a, "first tick should select task A");

    // Simulate the Exit syscall via dispatch (same path as syscall_rust_dispatch).
    let ret = kaos_kernel::syscall::dispatch(
        kaos_kernel::syscall::SyscallId::Exit as u64,
        0, // exit_code
        0,
        0,
        0,
    );
    assert!(
        ret == kaos_kernel::syscall::SYSCALL_OK,
        "Exit syscall must return SYSCALL_OK"
    );
    assert!(
        sched::task_state(task_a) == Some(TaskState::Zombie),
        "Exit syscall must mark the current task as Zombie"
    );

    // Next tick: zombie A is skipped and reaped, task B selected.
    current = sched::on_timer_tick(current);
    assert!(
        current == frame_b,
        "after Exit syscall, zombie task must be skipped"
    );

    // Zombie slot has been reaped.
    assert!(
        sched::task_state(task_a).is_none(),
        "zombie task must be reaped on subsequent tick"
    );
}
