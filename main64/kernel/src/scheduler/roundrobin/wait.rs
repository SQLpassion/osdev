//! Task blocking, unblocking, and exit waiting queues.

use super::manager::remove_task;
use super::types::TaskState;
use super::{
    arch_callbacks, current_task_id, is_running, task_frame_ptr, with_scheduler, yield_now,
};
use crate::sync::waitqueue::WaitQueue;
use crate::sync::waitqueue_adapter;

/// Wait queue for tasks blocked in `wait_for_task_exit`.
pub(crate) static TASK_EXIT_WAITQUEUE: WaitQueue = WaitQueue::new();

/// Marks the task in `task_id` as `TaskState::Blocked`.
///
/// A blocked task is skipped by the round-robin selector until it is
/// unblocked via `unblock_task`.
pub fn block_task(task_id: usize) {
    with_scheduler(|meta| {
        if task_id < meta.slots.len()
            && meta.slots[task_id].used
            && meta.slots[task_id].state != TaskState::Blocked
        {
            meta.slots[task_id].state = TaskState::Blocked;
        }
    });
}

/// Marks a previously blocked task as `TaskState::Ready`.
///
/// Safe to call from IRQ context (the scheduler spinlock handles
/// interrupt masking internally).
pub fn unblock_task(task_id: usize) {
    with_scheduler(|meta| {
        if task_id < meta.slots.len()
            && meta.slots[task_id].used
            && meta.slots[task_id].state == TaskState::Blocked
        {
            meta.slots[task_id].state = TaskState::Ready;
        }
    });
}

/// Terminates `task_id`, removing it from the run queue and freeing its slot.
///
/// The task's stack is deferred for freeing on the next timer tick so that
/// stale frame pointers can still be detected by the scheduler.
///
/// Returns `true` if the task existed and was removed.
pub fn terminate_task(task_id: usize) -> bool {
    // Snapshot callbacks before entering the scheduler lock to avoid
    // nested lock acquisition (`SCHED` -> `SCHED_ARCH_CALLBACKS`).
    let callbacks = arch_callbacks();
    let removed = with_scheduler(|meta| remove_task(meta, task_id, callbacks));

    // Step 1: Wake tasks that are blocked in `wait_for_task_exit`.
    // Wake-all is safe: each waiter re-checks its own task-id predicate and
    // sleeps again when its target is still alive.
    if removed {
        waitqueue_adapter::wake_all_multi(&TASK_EXIT_WAITQUEUE);
    }

    removed
}

/// Waits cooperatively until `task_id` is no longer present in the scheduler.
///
/// This is intended for foreground command flows (for example REPL `exec`)
/// that need to block the caller until a spawned task has terminated.
///
/// Behavior:
/// - if `task_id` is already absent, this returns immediately,
/// - otherwise this repeatedly yields so normal scheduler ticks can progress.
pub fn wait_for_task_exit(task_id: usize) {
    // Step 1: Fast path for already-absent targets.
    if task_frame_ptr(task_id).is_none() {
        return;
    }

    // Step 2: In scheduled task context, use wait-queue blocking instead of
    // spin-yield polling to avoid burning CPU while waiting.
    if is_running() {
        if let Some(waiter_task_id) = current_task_id() {
            // Self-wait cannot make progress through blocking because the target
            // would be the currently blocked task itself. Keep the historical
            // cooperative poll behavior for this edge case.
            if waiter_task_id == task_id {
                wait_for_task_exit_with(task_id, |id| task_frame_ptr(id).is_some(), yield_now);
                return;
            }

            loop {
                // Step 3: Atomically recheck liveness, register on queue, and block.
                let outcome =
                    waitqueue_adapter::sleep_if_multi(&TASK_EXIT_WAITQUEUE, waiter_task_id, || {
                        task_frame_ptr(task_id).is_some()
                    });

                // Step 4: Predicate false means target already exited; return.
                if !outcome.should_yield() {
                    return;
                }

                // Step 5: Blocked (or queue OOM degradation) path yields once.
                yield_now();
            }
        }
    }

    // Step 6: Fallback for non-scheduler contexts (boot/tests).
    wait_for_task_exit_with(task_id, |id| task_frame_ptr(id).is_some(), yield_now);
}

/// Generic wait helper behind `wait_for_task_exit`.
///
/// `is_task_alive` must report whether `task_id` is still present.
/// `yield_once` must provide one cooperative scheduling opportunity.
///
/// Primarily exposed to keep the wait-loop contract directly testable without
/// requiring real interrupt-driven context switches in tests.
pub fn wait_for_task_exit_with<FAlive, FYield>(
    task_id: usize,
    mut is_task_alive: FAlive,
    mut yield_once: FYield,
) where
    FAlive: FnMut(usize) -> bool,
    FYield: FnMut(),
{
    // Foreground wait policy:
    // - poll liveness,
    // - yield between polls so the target task can run and eventually exit.
    while is_task_alive(task_id) {
        yield_once();
    }
}
