//! Scheduler-aware adapter functions for wait-queue primitives.
//!
//! This module owns the coupling between wait-queue registration and scheduler
//! state transitions (`Blocked`/`Ready`).

use crate::arch::interrupts;
use crate::scheduler;

use super::singlewaitqueue::SingleWaitQueue;
use super::waitqueue::WaitQueue;

/// Conditionally blocks `task_id` on a multi-waiter queue.
///
/// Returns `true` when the task was actually blocked and should yield.
pub fn sleep_if_multi<const N: usize>(
    queue: &WaitQueue<N>,
    task_id: usize,
    should_block: impl FnOnce() -> bool,
) -> bool {
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();

    let mut blocked = should_block();
    if blocked {
        let registered = queue.register_waiter(task_id);
        if registered {
            scheduler::block_task(task_id);
        } else {
            blocked = false;
        }
    } else {
        queue.clear_waiter(task_id);
    }

    if were_enabled {
        interrupts::enable();
    }

    blocked
}

/// Wakes all waiters in a multi-waiter queue.
pub fn wake_all_multi<const N: usize>(queue: &WaitQueue<N>) {
    queue.wake_all(scheduler::unblock_task);
}

/// Conditionally blocks `task_id` on a single-waiter queue.
///
/// Returns `true` when the task was actually blocked and should yield.
pub fn sleep_if_single(
    queue: &SingleWaitQueue,
    task_id: usize,
    should_block: impl FnOnce() -> bool,
) -> bool {
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();

    let mut blocked = should_block();
    if blocked {
        let registered = queue.register_waiter(task_id);
        if registered {
            scheduler::block_task(task_id);
        } else {
            blocked = false;
        }
    } else {
        queue.clear_waiter(task_id);
    }

    if were_enabled {
        interrupts::enable();
    }

    blocked
}

/// Wakes all waiters in a single-waiter queue (at most one task).
pub fn wake_all_single(queue: &SingleWaitQueue) {
    queue.wake_all(scheduler::unblock_task);
}
