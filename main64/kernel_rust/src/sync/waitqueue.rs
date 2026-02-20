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
    state: SpinLock<WaitQueueState>,
}

struct WaitQueueState {
    waiters: Vec<usize>,
    wake_scratch: Vec<usize>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            // `Vec::new()` and `SpinLock::new()` are both `const fn`,
            // so `WaitQueue` can be used as a `static`.
            state: SpinLock::new(WaitQueueState {
                waiters: Vec::new(),
                wake_scratch: Vec::new(),
            }),
        }
    }

    /// Registers `task_id` as a waiter.
    ///
    /// Returns `false` only on heap-allocation failure (OOM).
    /// Re-registration of the same `task_id` is idempotent.
    pub fn register_waiter(&self, task_id: usize) -> bool {
        let mut state = self.state.lock();
        if state.waiters.contains(&task_id) {
            return true; // already registered – idempotent
        }
        // Use try_reserve to avoid a panic (via the alloc-error handler)
        // inside the spinlock, where interrupts are already disabled.
        if state.waiters.try_reserve(1).is_err() {
            return false; // OOM — caller treats this as QueueFull
        }
        state.waiters.push(task_id);
        true
    }

    /// Removes the registration for `task_id`.
    pub fn clear_waiter(&self, task_id: usize) {
        self.state.lock().waiters.retain(|&id| id != task_id);
    }

    /// Atomically drains every registered waiter and calls `wake` for each.
    ///
    /// Step 1: take ownership of the full waiter list under one lock hold.
    /// Step 2: release the lock and wake all drained waiters.
    ///
    /// This avoids repeated lock/unlock churn for large waiter sets and keeps
    /// scheduler wakeup work outside the queue lock.
    pub fn wake_all(&self, mut wake: impl FnMut(usize)) {
        // Step 1: swap waiters into scratch under one lock hold and move the
        // batch out for wake processing.
        let mut drained_waiters = {
            let mut state = self.state.lock();
            state.wake_scratch.clear();
            let WaitQueueState {
                waiters,
                wake_scratch,
            } = &mut *state;
            core::mem::swap(waiters, wake_scratch);
            core::mem::take(&mut state.wake_scratch)
        };

        // Step 2: wake drained waiters after releasing the queue lock.
        for task_id in drained_waiters.iter().copied() {
            wake(task_id);
        }

        // Step 3: clear and recycle the drained batch capacity for the next cycle.
        drained_waiters.clear();
        let mut state = self.state.lock();
        if state.wake_scratch.capacity() < drained_waiters.capacity() {
            state.wake_scratch = drained_waiters;
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
