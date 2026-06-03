//! Multi-waiter task wait-queue primitive with OOM safety and lock-free execution steps.
//!
//! This module provides a thread-safe `WaitQueue` that tracks multiple waiters.
//! It decouples queue management from the scheduler, which is driven by adapter functions
//! in `waitqueue_adapter.rs`.
//!
//! ## Design Architecture and State Transitions
//!
//! Waiters are tracked dynamically using a heap-allocated `Vec<usize>` containing task IDs.
//! An internal `SpinLock` serializes concurrent accesses and disables interrupts, making the structure
//! safe to use within interrupt handlers.
//!
//! ```text
//!          [Task Registers]
//!                 │
//!                 ▼
//!        ┌─────────────────┐
//!        │  waiters (Vec)  │◄───────── [clear_waiter]
//!        └────────┬────────┘
//!                 │
//!                 ▼ [wake_all: Swap waiters list under lock]
//!        ┌─────────────────┐
//!        │  wake_scratch   │ (Drained waiters list)
//!        └────────┬────────┘
//!                 │
//!                 ▼ [Lock Released: unblock_task callback executed]
//!      [All Waiter Tasks Woken]
//! ```
//!
//! ## OOM Protection and Spinlock Safety
//!
//! Calling `register_waiter` locks the internal state. Because the lock disables CPU interrupts,
//! running out of memory (OOM) during dynamic vector expansion would panic and crash the kernel.
//! To prevent this, `WaitQueue` uses `try_reserve(1)` to preallocate space:
//! - On OOM, it returns `false` early, allowing the adapter/caller to yield and handle pressure.
//!
//! ## The Double-Buffer Recycling Wake Strategy (`wake_all`)
//!
//! To keep scheduler wakeup work outside the queue lock:
//! 1. **Swap State:** Swaps the active `waiters` vector into `wake_scratch` under one lock hold.
//! 2. **Process Wakeups:** Releases the lock and calls the `wake` closure for each task ID.
//! 3. **Recycle Buffer:** Re-locks the state and moves the emptied vector back into `wake_scratch`
//!    if its capacity exceeds the existing scratch buffer, saving future allocations.

extern crate alloc;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;

/// A thread-safe, multi-waiter task wait-queue utilizing interrupt-safe spinlocks.
///
/// Under the hood, this structure manages a vector of task IDs protected by a `SpinLock`.
/// It implements `Sync` and `Send` to allow multiple CPUs to safely enqueue and wake waiters.
pub struct WaitQueue {
    state: SpinLock<WaitQueueState>,
}

struct WaitQueueState {
    waiters: Vec<usize>,
    wake_scratch: Vec<usize>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        // Step 1: Construct the WaitQueueState wrapped inside a SpinLock.
        // `Vec::new()` and `SpinLock::new()` are both `const fn`,
        // so `WaitQueue` can be used as a `static`.
        Self {
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
        // Step 1: Acquire the spinlock protecting the WaitQueueState.
        // This disables interrupts on the current CPU during the registration lock duration.
        let mut state = self.state.lock();

        // Step 2: Check if the waiter is already registered.
        // This enforces idempotency of waiter registration.
        if state.waiters.contains(&task_id) {
            return true;
        }

        // Step 3: Ensure there is capacity in the vector to append the waiter.
        // Use `try_reserve` to avoid a panic (via the alloc-error handler)
        // inside the spinlock, where interrupts are already disabled.
        if state.waiters.try_reserve(1).is_err() {
            return false; // OOM — caller treats this as QueueFull
        }

        // Step 4: Push the task_id into the waiters list.
        state.waiters.push(task_id);
        true
    }

    /// Removes the registration for `task_id`.
    pub fn clear_waiter(&self, task_id: usize) {
        // Step 1: Acquire the spinlock and retain only those waiters that do not match `task_id`.
        // This removes the registration of this task from the queue.
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
        // Step 1: Acquire the spinlock and swap the waiters vector with our scratch buffer.
        // This drains the list of waiters quickly under the protection of the lock.
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

        // Step 2: Wake drained waiters after releasing the queue lock.
        // This avoids holding the waitqueue spinlock while performing scheduler wakeups.
        for task_id in drained_waiters.iter().copied() {
            wake(task_id);
        }

        // Step 3: Clear and recycle the drained batch capacity for the next cycle.
        // We re-acquire the lock and put the larger capacity vector back in `wake_scratch`
        // to avoid future allocations.
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
