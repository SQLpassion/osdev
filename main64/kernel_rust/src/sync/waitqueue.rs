//! Multi-waiter wait queue with wake-all semantics.
//!
//! Modelled after the Linux kernel wait queue pattern: tasks register
//! themselves as waiters, are marked [`Blocked`] in the scheduler, and
//! are woken (set back to [`Ready`]) when an event occurs.
//!
//! [`Blocked`]: crate::scheduler::TaskState::Blocked
//! [`Ready`]:   crate::scheduler::TaskState::Ready

use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::interrupts;
use crate::scheduler;

/// Maximum number of concurrent waiters (matches scheduler MAX_TASKS).
const MAX_WAITERS: usize = 8;

/// A wait queue that supports multiple blocked tasks with wake-all semantics.
///
/// # Usage
///
/// **Sleeping** (task context):
/// ```ignore
/// if !data_available() {
///     WAIT_QUEUE.sleep(my_task_id);
///     scheduler::yield_now(); // reschedule — this task is now Blocked
/// }
/// ```
///
/// **Waking** (IRQ or task context):
/// ```ignore
/// produce_data();
/// WAIT_QUEUE.wake_all(); // all waiters become Ready
/// ```
pub struct WaitQueue {
    waiters: [AtomicBool; MAX_WAITERS],
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: [
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            ],
        }
    }

    /// Register the calling task as a waiter and mark it as [`Blocked`] in the
    /// scheduler.
    ///
    /// Interrupts are disabled for the combined waiter-registration +
    /// block-task sequence to prevent lost wakeups: if an IRQ fired between
    /// registering the waiter flag and calling `block_task`, the corresponding
    /// `wake_all` could observe the flag, clear it, and call `unblock_task`
    /// *before* the task is actually blocked — leaving it blocked forever.
    ///
    /// After this call the caller should invoke [`scheduler::yield_now()`] to
    /// hand the CPU to the next ready task.
    ///
    /// [`Blocked`]: crate::scheduler::TaskState::Blocked
    #[allow(dead_code)]
    pub fn sleep(&self, task_id: usize) {
        let were_enabled = interrupts::are_enabled();
        interrupts::disable();

        self.waiters[task_id].store(true, Ordering::Release);
        scheduler::block_task(task_id);

        if were_enabled {
            interrupts::enable();
        }
    }

    /// Conditionally block the calling task: the `should_block` predicate is
    /// evaluated with **interrupts disabled** so that no IRQ can slip in between
    /// the check and the state change.  This prevents the classic lost-wakeup
    /// race where the producer fires `wake_all` between the caller's empty-check
    /// and the `sleep` call.
    ///
    /// Returns `true` if the task was blocked (caller should `yield_now()`),
    /// `false` if the predicate returned `false` (no sleep was necessary).
    pub fn sleep_if(&self, task_id: usize, should_block: impl FnOnce() -> bool) -> bool {
        let were_enabled = interrupts::are_enabled();
        interrupts::disable();

        let blocked = should_block();
        if blocked {
            self.waiters[task_id].store(true, Ordering::Release);
            scheduler::block_task(task_id);
        }

        if were_enabled {
            interrupts::enable();
        }
        blocked
    }

    /// Wake **all** tasks currently registered as waiters.
    ///
    /// Each waiter is set back to [`Ready`] in the scheduler.  Woken tasks
    /// will compete for the available data on their next scheduling quantum;
    /// those that find no data simply sleep again (thundering-herd pattern).
    ///
    /// Safe to call from both IRQ context and task context.
    ///
    /// [`Ready`]: crate::scheduler::TaskState::Ready
    pub fn wake_all(&self) {
        for i in 0..MAX_WAITERS {
            if self.waiters[i].swap(false, Ordering::AcqRel) {
                scheduler::unblock_task(i);
            }
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: All fields are atomic — no shared mutable state.
unsafe impl Sync for WaitQueue {}
unsafe impl Send for WaitQueue {}
