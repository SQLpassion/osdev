//! Single-waiter task queue primitive.
//!
//! This module provides `SingleWaitQueue`, a lightweight synchronization primitive
//! designed for top-half/bottom-half driver architectures where exactly one task
//! waits for events (e.g., a dedicated disk or keyboard worker thread).
//!
//! Unlike `WaitQueue`, it requires no heap allocation and contains no locks.
//!
//! ## Operation Flow
//!
//! The waiter slot transitions between `NO_WAITER` (sentinel value representing an empty slot)
//! and a specific `task_id` (representing the blocked thread).
//!
//! ```text
//!            Empty Slot (NO_WAITER)
//!                  │
//!                  ▼ [register_waiter(task_id)]
//!            Occupied (task_id)
//!            ┌─────┴─────┐
//! [clear_waiter]         ▼ [wake_all(unblock)]
//!            └────► Empty Slot (NO_WAITER)
//! ```
//!
//! ## Memory Ordering and Synchronization
//!
//! All operations utilize atomic memory orderings to synchronize waiter status
//! without locks:
//!
//! * **Registration:** Uses `Ordering::AcqRel` on success. This publishes the waiter's registration
//!   to other CPUs and acquires the latest updates.
//! * **Clearing:** Uses `Ordering::AcqRel` on compare-exchange to atomically free the slot
//!   if it currently holds the expected `task_id`.
//! * **Waking:** Uses `Ordering::AcqRel` on swap to retrieve the waiter while clearing the slot.

use core::sync::atomic::{AtomicUsize, Ordering};

const NO_WAITER: usize = usize::MAX;

/// A lock-free, zero-allocation wait-queue that supports at most one waiter.
///
/// Under the hood, this uses an `AtomicUsize` containing either a valid task ID
/// or the sentinel `usize::MAX` (`NO_WAITER`). It implements `Sync` and `Send`
/// because all concurrent access is fully synchronized via lock-free atomic pathways.
pub struct SingleWaitQueue {
    waiter: AtomicUsize,
}

impl SingleWaitQueue {
    pub const fn new() -> Self {
        // Step 1: Initialize the atomic waiter slot with the NO_WAITER sentinel to represent no registered waiter.
        Self {
            waiter: AtomicUsize::new(NO_WAITER),
        }
    }

    /// Registers `task_id` as the current waiter.
    ///
    /// Returns `false` when a different waiter is already registered.
    pub fn register_waiter(&self, task_id: usize) -> bool {
        // Step 1: Validate that the incoming `task_id` does not conflict with our sentinel value.
        // `usize::MAX` is the NO_WAITER sentinel — passing it would silently
        // be treated as "slot already empty" and corrupt queue state.
        debug_assert!(
            task_id != NO_WAITER,
            "register_waiter: task_id == usize::MAX collides with NO_WAITER sentinel"
        );

        // Step 2: Attempt to transition the waiter slot from NO_WAITER to the target task_id.
        // - `Ordering::AcqRel` on success ensures updates are synchronized correctly.
        // - If successful, the task is now registered as the exclusive waiter (return true).
        // - If it fails because the slot is already set to `task_id`, this is idempotent (return true).
        // - If it fails because another task is registered, return false.
        match self
            .waiter
            .compare_exchange(NO_WAITER, task_id, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            Err(existing) => existing == task_id,
        }
    }

    /// Clears `task_id` as current waiter.
    pub fn clear_waiter(&self, task_id: usize) {
        // Step 1: Atomically clear the waiter slot back to NO_WAITER, but ONLY if it is currently set to `task_id`.
        // This prevents a late-running clear call from accidentally clearing a newly-registered waiter.
        let _ =
            self.waiter
                .compare_exchange(task_id, NO_WAITER, Ordering::AcqRel, Ordering::Acquire);
    }

    /// Wakes the currently registered waiter (if any) and clears the slot.
    pub fn wake_all(&self, mut wake: impl FnMut(usize)) {
        // Step 1: Swap out the registered waiter with NO_WAITER in a single atomic operation.
        // This ensures that we take ownership of the waiter and clear the slot atomically.
        let task_id = self.waiter.swap(NO_WAITER, Ordering::AcqRel);

        // Step 2: If a valid waiter was registered, invoke the wake callback to unblock it.
        if task_id != NO_WAITER {
            wake(task_id);
        }
    }
}

impl Default for SingleWaitQueue {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: All fields are atomic — no shared mutable state.
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
unsafe impl Sync for SingleWaitQueue {}
unsafe impl Send for SingleWaitQueue {}
