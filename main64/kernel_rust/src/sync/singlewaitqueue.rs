//! Single-waiter queue primitive.
//!
//! Useful for producer/worker top-half/bottom-half paths where only one task
//! can wait at a time.

use core::sync::atomic::{AtomicUsize, Ordering};

const NO_WAITER: usize = usize::MAX;

pub struct SingleWaitQueue {
    waiter: AtomicUsize,
}

impl SingleWaitQueue {
    pub const fn new() -> Self {
        Self {
            waiter: AtomicUsize::new(NO_WAITER),
        }
    }

    /// Registers `task_id` as the current waiter.
    ///
    /// Returns `false` when a different waiter is already registered.
    pub fn register_waiter(&self, task_id: usize) -> bool {
        // `usize::MAX` is the NO_WAITER sentinel — passing it would silently
        // be treated as "slot already empty" and corrupt queue state.
        debug_assert!(
            task_id != NO_WAITER,
            "register_waiter: task_id == usize::MAX collides with NO_WAITER sentinel"
        );

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
        let _ =
            self.waiter
                .compare_exchange(task_id, NO_WAITER, Ordering::AcqRel, Ordering::Acquire);
    }

    /// Wakes the currently registered waiter (if any) and clears the slot.
    pub fn wake_all(&self, mut wake: impl FnMut(usize)) {
        let task_id = self.waiter.swap(NO_WAITER, Ordering::AcqRel);
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
