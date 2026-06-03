//! Scheduler-aware adapter layer for task wait-queue primitives.
//!
//! This module binds wait-queue structures (`WaitQueue`, `SingleWaitQueue`) to the actual
//! task scheduler. It governs task state transitions between `Blocked` and `Ready`.
//!
//! ## The Lost Wakeup Problem & Atomic Evaluation
//!
//! A classic race condition in operating systems occurs when a task checks a condition,
//! determines it must block, but gets interrupted *before* enqueuing itself.
//! If the event/interrupt fires and calls `wake_all` during this gap, the wakeup is lost,
//! and the task will sleep indefinitely when it finally calls `block_task`:
//!
//! ```text
//! Task Context                          Interrupt/Wake Context
//!   │                                     │
//!   ├──► 1. Evaluate condition (true)     │
//!   │                                     ├──► 2. Event occurs (data arrives)
//!   │                                     ├──► 3. wake_all() called (No one waiting yet!)
//!   │                                     │
//!   ├──► 4. Mark state as Blocked ────────┼──────┐
//!   │    (Yields CPU)                     │      │ (Blocked forever!)
//!   ▼                                     ▼      ▼
//! ```
//!
//! To prevent lost wakeups, `sleep_if_multi` and `sleep_if_single` execute the entire
//! condition-check and queue-registration sequence under local CPU interrupt disablement.
//!
//! ## Execution Logic
//!
//! ```text
//!    [Start sleep_if_*]
//!            │
//!            ▼ [Disable Interrupts]
//!     Check Condition? ──(False)──► [Clear Waiter] ──► [Restore Interrupts] ──► ConditionFalse
//!            │ (True)
//!            ▼
//!    Try Register Waiter? ──(Fail)──► [Restore Interrupts] ──► QueueFull (Yield requested)
//!            │ (Success)
//!            ▼
//!     [Mark Blocked in Scheduler]
//!            │
//!            ▼
//!    [Restore Interrupts] ──► Blocked (Yield requested)
//! ```

use crate::arch::interrupts;
use crate::scheduler;

use super::singlewaitqueue::SingleWaitQueue;
use super::waitqueue::WaitQueue;

/// The outcome of a conditional sleep registration attempt.
///
/// Enables callers to decide whether to yield CPU execution to another task
/// without needing to re-evaluate the original event condition.
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
    // Step 1: Disable interrupts locally to ensure atomic evaluation of the blocking condition
    // and registration of the task. This prevents race conditions where an interrupt wakes up the task
    // before it is fully registered and blocked.
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();

    // Step 2: Evaluate whether the task needs to block.
    let outcome = if should_block() {
        // Step 2a: Try to register the task in the multi-waiter queue.
        if queue.register_waiter(task_id) {
            // Step 2b: Registration succeeded. Now mark the task as Blocked in the scheduler.
            scheduler::block_task(task_id);
            SleepOutcome::Blocked
        } else {
            // Step 2c: Registration failed (OOM/QueueFull). Return QueueFull so the caller can yield.
            SleepOutcome::QueueFull
        }
    } else {
        // Step 2d: The condition is false, meaning the event has already occurred or data is available.
        // Clear any old registration for safety and don't block.
        queue.clear_waiter(task_id);
        SleepOutcome::ConditionFalse
    };

    // Step 3: Restore the original interrupt state.
    if were_enabled {
        interrupts::enable();
    }

    outcome
}

/// Wakes all waiters in a multi-waiter queue.
pub fn wake_all_multi(queue: &WaitQueue) {
    // Step 1: Drain the waitqueue and unblock each task ID in the scheduler.
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
    // Step 1: Disable interrupts locally to ensure atomic evaluation of the blocking condition
    // and registration of the task. This prevents race conditions where an interrupt wakes up the task
    // before it is fully registered and blocked.
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();

    // Step 2: Evaluate whether the task needs to block.
    let outcome = if should_block() {
        // Step 2a: Try to register the task in the single-waiter queue.
        if queue.register_waiter(task_id) {
            // Step 2b: Registration succeeded. Mark the task as Blocked in the scheduler.
            scheduler::block_task(task_id);
            SleepOutcome::Blocked
        } else {
            // Step 2c: Registration failed (another waiter exists). Return QueueFull so the caller yields.
            SleepOutcome::QueueFull
        }
    } else {
        // Step 2d: The condition is false. Clear any old registration and don't block.
        queue.clear_waiter(task_id);
        SleepOutcome::ConditionFalse
    };

    // Step 3: Restore the original interrupt state.
    if were_enabled {
        interrupts::enable();
    }

    outcome
}

/// Wakes all waiters in a single-waiter queue (at most one task).
pub fn wake_all_single(queue: &SingleWaitQueue) {
    // Step 1: Drain the single-waiter slot and unblock the task in the scheduler if present.
    queue.wake_all(scheduler::unblock_task);
}
