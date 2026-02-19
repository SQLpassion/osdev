//! Scheduler-agnostic wait-queue primitives.
//!
//! This module only tracks waiter registration.  Blocking/unblocking tasks is
//! handled by adapter functions in `waitqueue_adapter.rs`.

extern crate alloc;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;

/// Multi-waiter queue with wake-all semantics.
///
/// Waiters are stored in a heap-allocated `Vec`, so capacity is bounded only
/// by available heap memory.  Any `task_id` value is valid — there is no
/// sentinel that limits the usable ID range.
///
/// Concurrent access is serialised by an internal `SpinLock`.  The lock
/// disables interrupts while held, so this type is safe to use from both
/// task context and IRQ handlers.
pub struct WaitQueue {
    waiters: SpinLock<Vec<usize>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            // `Vec::new()` and `SpinLock::new()` are both `const fn`,
            // so `WaitQueue` can be used as a `static`.
            waiters: SpinLock::new(Vec::new()),
        }
    }

    /// Registers `task_id` as a waiter.
    ///
    /// Returns `false` only on heap-allocation failure (OOM).
    /// Re-registration of the same `task_id` is idempotent.
    pub fn register_waiter(&self, task_id: usize) -> bool {
        let mut w = self.waiters.lock();
        if w.contains(&task_id) {
            return true; // already registered – idempotent
        }
        // Use try_reserve to avoid a panic (via the alloc-error handler)
        // inside the spinlock, where interrupts are already disabled.
        if w.try_reserve(1).is_err() {
            return false; // OOM — caller treats this as QueueFull
        }
        w.push(task_id);
        true
    }

    /// Removes the registration for `task_id`.
    pub fn clear_waiter(&self, task_id: usize) {
        self.waiters.lock().retain(|&id| id != task_id);
    }

    /// Atomically drains every registered waiter and calls `wake` for each.
    ///
    /// The waiter list is drained under the internal lock via `mem::take`
    /// (zero allocation), then `wake` is called for each entry *outside* the
    /// lock.  This avoids holding the WaitQueue lock while acquiring the
    /// scheduler lock inside `wake`.
    pub fn wake_all(&self, mut wake: impl FnMut(usize)) {
        let to_wake = core::mem::take(&mut *self.waiters.lock());
        for task_id in to_wake {
            wake(task_id);
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
