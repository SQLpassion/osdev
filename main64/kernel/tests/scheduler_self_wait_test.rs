//! Regression test for the scheduler self-wait guard (issue #46).
//!
//! `wait_for_task_exit`'s self-wait fast path is meant to detect a task
//! waiting on its own packed task id and fall back to a cooperative
//! spin/yield loop, since blocking such a task on `TASK_EXIT_WAITQUEUE`
//! would never be woken again (only `terminate_task`/`remove_task` acting on
//! some *other* task ever wakes that queue, and this task obviously can
//! never terminate itself while it is stuck waiting on itself).
//!
//! Before the fix, the self-wait detection compared a *packed* task id
//! (`current_task_id()`) against a *bare* slot index, so the comparison was
//! always false. A self-waiting task therefore fell through to the
//! blocking-queue path instead, which marks it `TaskState::Blocked` and
//! leaves it there forever (nothing wakes it specifically). This test spawns
//! a task that waits on its own id and asserts it is *never* observed as
//! `TaskState::Blocked` while doing so: the fixed cooperative-poll path never
//! transitions the task's scheduler state, while the pre-fix blocking path
//! does so on its very first loop iteration.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, TaskState};

/// Set by `self_waiter_task` right before it enters `wait_for_task_exit` on
/// its own packed task id, so the orchestrator knows when subsequent
/// scheduler-state observations are meaningful.
static SELF_WAIT_ENTERED: AtomicU64 = AtomicU64::new(0);

/// Set by the orchestrator if it ever observes the self-waiting task in
/// `TaskState::Blocked` -- the signature of the pre-fix bug.
static OBSERVED_BLOCKED: AtomicU64 = AtomicU64::new(0);

/// Number of state observations the orchestrator successfully performed
/// while the self-waiting task was still alive; used to make sure the test
/// actually exercised the loop rather than trivially passing.
static OBSERVATION_ROUNDS: AtomicU64 = AtomicU64::new(0);

/// Final pass/fail latch consumed by the `#[test_case]` assertion.
static TEST_SUCCESS: AtomicU64 = AtomicU64::new(0);

/// Entry point for the self-wait integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Step 1: Initialize memory and interrupt subsystems required by the scheduler.
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    // Step 2: Initialize the scheduler and spawn the orchestrator task. The
    // orchestrator owns the whole test flow and invokes the standard test
    // harness once it has driven the self-wait scenario to completion.
    sched::init();
    sched::spawn_kernel_task(orchestrator_task).expect("orchestrator task should spawn");
    sched::start();

    // Step 3: Enable periodic timer interrupts so real preemption backs the
    // orchestrator/self-waiter round robin. Spawned task bodies only ever
    // execute for real via the interrupt-driven context-switch path; without
    // this, only frame pointers would be selectable, never actual code.
    interrupts::init_periodic_timer(250);
    interrupts::enable();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// A task that deliberately waits on its own packed task id.
///
/// This can never make progress on its own (the target of the wait is the
/// caller itself, which cannot exit while it is busy waiting on itself), so
/// the orchestrator forcibly terminates it after observing scheduler state.
extern "C" fn self_waiter_task() -> ! {
    // Step 1: Resolve our own packed task identifier the same way any real
    // caller would have to (there is no other way to obtain "your own" id).
    let self_id = sched::current_task_id().expect("self_waiter must be the running task");

    // Step 2: Signal the orchestrator that we are about to enter the wait so
    // it knows subsequent scheduler-state observations are meaningful.
    SELF_WAIT_ENTERED.store(1, Ordering::Release);

    // Step 3: Wait on our own id. Fixed behavior: falls into the cooperative
    // spin/yield fast path and keeps yielding forever without ever changing
    // this task's scheduler state. Buggy (pre-fix) behavior: falls through
    // to the blocking-queue path, which marks this task `Blocked` on the
    // very first iteration and never wakes it again.
    sched::wait_for_task_exit(self_id);

    // Unreachable: the orchestrator terminates this task before the wait
    // could ever return (it is fundamentally unable to finish on its own).
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn orchestrator_task() -> ! {
    // Step 1: Reset state so the test is idempotent if re-run in place.
    SELF_WAIT_ENTERED.store(0, Ordering::Release);
    OBSERVED_BLOCKED.store(0, Ordering::Release);
    OBSERVATION_ROUNDS.store(0, Ordering::Release);
    TEST_SUCCESS.store(0, Ordering::Release);

    // Step 2: Spawn the self-waiting task; its packed id is only known to us
    // via the spawn return value (mirroring how a real caller would track
    // "a task id I want to wait on" -- just not its *own* id, which is the
    // scenario under test here).
    let self_id = sched::spawn_kernel_task(self_waiter_task).expect("self_waiter should spawn");

    // Step 3: Yield until the self-waiter has signalled it is inside the
    // wait call. Bounded so a scheduling regression fails loudly instead of
    // hanging the whole test binary (the outer QEMU run is also time-boxed).
    let mut entry_spins: u32 = 0;
    while SELF_WAIT_ENTERED.load(Ordering::Acquire) == 0 {
        sched::yield_now();
        entry_spins += 1;
        assert!(
            entry_spins < 64,
            "self_waiter never signalled entry into wait_for_task_exit"
        );
    }

    // Step 4: Repeatedly hand the CPU to the self-waiter and inspect its
    // scheduler state afterwards. The fixed cooperative-poll path never
    // blocks this task, so its state must never be observed as `Blocked`;
    // the pre-fix bug blocks it on the very first loop iteration and never
    // unblocks it again (only an unrelated task's exit wakes the queue, and
    // nothing else exits in this test).
    for _ in 0..8 {
        sched::yield_now();
        match sched::task_state(self_id) {
            Some(TaskState::Blocked) => {
                OBSERVED_BLOCKED.store(1, Ordering::Release);
            }
            Some(_) => {}
            None => panic!("self_waiter must still be alive; it never exits on its own"),
        }
        OBSERVATION_ROUNDS.fetch_add(1, Ordering::AcqRel);
    }

    // Step 5: Clean up the self-waiter now that observation is complete. Its
    // own wait loop can never terminate on its own, so this is the only way
    // to reap it and free its stack.
    assert!(
        sched::terminate_task(self_id),
        "orchestrator must be able to terminate the self-waiting task"
    );

    // Step 6: Evaluate the regression assertions now that the scenario ran
    // to completion under real preemption.
    assert_eq!(
        OBSERVED_BLOCKED.load(Ordering::Acquire),
        0,
        "self-wait must never park the waiting task in TaskState::Blocked \
         (that indicates it fell through to the unwakeable blocking-queue path)"
    );
    assert!(
        OBSERVATION_ROUNDS.load(Ordering::Acquire) >= 8,
        "test must have actually observed scheduler state across multiple rounds"
    );

    TEST_SUCCESS.store(1, Ordering::Release);

    // Step 7: Run the standard test harness from task context so the runner
    // sees the usual "Total/Passed" summary and exits QEMU cleanly.
    test_main();

    // test_main() never returns; it exits QEMU directly.
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: a task waiting on its own packed task id takes the cooperative
/// spin/yield fast path and is never left parked in `TaskState::Blocked`.
/// Given: A scheduler task that calls `wait_for_task_exit` with its own
/// packed task id (obtained via `current_task_id()`), while a second task
/// (the orchestrator) drives real preemption and repeatedly inspects it.
/// When: The orchestrator yields to the self-waiting task across several
/// rounds and records whether it was ever observed as `TaskState::Blocked`.
/// Then: The self-waiting task is never observed as `Blocked`, proving the
/// self-wait detection correctly compares like-for-like (bare slot vs. bare
/// slot) instead of a packed id against a bare slot, which used to always
/// fall through to the blocking-queue path and park the task unwakeably.
/// Failure Impact: A regression here reintroduces a scheduler internal API
/// footgun where any future caller of `wait_for_task_exit(own_task_id)`
/// would deadlock permanently instead of spinning cooperatively.
#[test_case]
fn test_self_wait_takes_cooperative_poll_path_never_blocks() {
    assert_eq!(
        TEST_SUCCESS.load(Ordering::Acquire),
        1,
        "self-wait scenario must complete and confirm cooperative-poll behavior"
    );
}
