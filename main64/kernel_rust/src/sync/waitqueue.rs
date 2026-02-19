//! Scheduler-agnostic wait-queue primitives.
//!
//! This module only tracks waiter registration.  Blocking/unblocking tasks is
//! handled by adapter functions in `waitqueue_adapter.rs`.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Sentinel value meaning "slot is empty".
const NO_WAITER: usize = usize::MAX;

/// Multi-waiter queue with wake-all semantics.
///
/// `N` is the maximum number of concurrently registered waiters.
/// Task IDs are stored as values in slots — not used as indices —
/// so any task_id is valid regardless of its magnitude.
pub struct WaitQueue<const N: usize> {
    slots: [AtomicUsize; N],
}

impl<const N: usize> WaitQueue<N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { AtomicUsize::new(NO_WAITER) }; N],
        }
    }

    /// Registers `task_id` as a waiter.
    ///
    /// Returns `false` only when all `N` slots are already occupied by other
    /// task IDs.  Re-registration of the same `task_id` is idempotent.
    ///
    /// On a single-core kernel this must be called with interrupts disabled so
    /// the scan-then-CAS sequence is not interrupted.
    pub fn register_waiter(&self, task_id: usize) -> bool {
        // Single pass: remember the first empty slot while checking for an
        // existing registration of this task_id.
        let mut first_empty: Option<&AtomicUsize> = None;
        for slot in self.slots.iter() {
            let current = slot.load(Ordering::Acquire);
            if current == task_id {
                return true; // already registered – idempotent
            }
            if current == NO_WAITER && first_empty.is_none() {
                first_empty = Some(slot);
            }
        }
        // Claim the first empty slot found.
        if let Some(slot) = first_empty {
            // With interrupts disabled (single-core) this CAS always succeeds;
            // kept for correctness on future SMP paths.
            slot.compare_exchange(NO_WAITER, task_id, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        } else {
            false // all N slots occupied
        }
    }

    /// Removes the registration for `task_id`.
    pub fn clear_waiter(&self, task_id: usize) {
        for slot in self.slots.iter() {
            if slot
                .compare_exchange(task_id, NO_WAITER, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Atomically drains every registered waiter and calls `wake` for each.
    pub fn wake_all(&self, mut wake: impl FnMut(usize)) {
        for slot in self.slots.iter() {
            let task_id = slot.swap(NO_WAITER, Ordering::AcqRel);
            if task_id != NO_WAITER {
                wake(task_id);
            }
        }
    }
}

impl<const N: usize> Default for WaitQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: All fields are atomic — no shared mutable state.
unsafe impl<const N: usize> Sync for WaitQueue<N> {}
unsafe impl<const N: usize> Send for WaitQueue<N> {}
