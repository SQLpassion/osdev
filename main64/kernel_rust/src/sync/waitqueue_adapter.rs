//! Scheduler-aware adapter functions for wait-queue primitives.
//!
//! This module owns the coupling between wait-queue registration and scheduler
//! state transitions (`Blocked`/`Ready`).

use crate::arch::interrupts;
use crate::scheduler;

use super::singlewaitqueue::SingleWaitQueue;
use super::waitqueue::WaitQueue;

/// Outcome of a conditional sleep attempt.
///
/// Returned by [`sleep_if_multi`] and [`sleep_if_single`] so callers can
/// decide whether to yield without rechecking the original condition.
pub enum SleepOutcome {
    /// Task was registered in the queue and blocked.
    /// Caller should call `yield_now()` to hand the CPU to another task.
    Blocked,

    /// The blocking condition was true but queue registration failed (OOM).
    /// Caller should call `yield_now()` to avoid busy-spinning while the
    /// allocator pressure resolves.
    QueueFull,

    /// The blocking condition was false — data is already available.
    /// Caller should NOT yield; the next loop iteration will consume the data.
    ConditionFalse,
}

impl SleepOutcome {
    /// Returns `true` when the caller should yield before retrying.
    ///
    /// Covers both the normal block path and the OOM-degradation path.
    pub fn should_yield(&self) -> bool {
        matches!(self, Self::Blocked | Self::QueueFull)
    }
}

/// Conditionally blocks `task_id` on a multi-waiter queue.
///
/// Returns the [`SleepOutcome`] so the caller can decide whether to yield.
pub fn sleep_if_multi(
    queue: &WaitQueue,
    task_id: usize,
    should_block: impl FnOnce() -> bool,
) -> SleepOutcome {
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();

    let outcome = if should_block() {
        if queue.register_waiter(task_id) {
            scheduler::block_task(task_id);
            SleepOutcome::Blocked
        } else {
            SleepOutcome::QueueFull
        }
    } else {
        queue.clear_waiter(task_id);
        SleepOutcome::ConditionFalse
    };

    if were_enabled {
        interrupts::enable();
    }

    outcome
}

/// Wakes all waiters in a multi-waiter queue.
pub fn wake_all_multi(queue: &WaitQueue) {
    queue.wake_all(scheduler::unblock_task);
}

/// Conditionally blocks `task_id` on a single-waiter queue.
///
/// Returns the [`SleepOutcome`] so the caller can decide whether to yield.
pub fn sleep_if_single(
    queue: &SingleWaitQueue,
    task_id: usize,
    should_block: impl FnOnce() -> bool,
) -> SleepOutcome {
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();

    let outcome = if should_block() {
        if queue.register_waiter(task_id) {
            scheduler::block_task(task_id);
            SleepOutcome::Blocked
        } else {
            SleepOutcome::QueueFull
        }
    } else {
        queue.clear_waiter(task_id);
        SleepOutcome::ConditionFalse
    };

    if were_enabled {
        interrupts::enable();
    }

    outcome
}

/// Wakes all waiters in a single-waiter queue (at most one task).
pub fn wake_all_single(queue: &SingleWaitQueue) {
    queue.wake_all(scheduler::unblock_task);
}
