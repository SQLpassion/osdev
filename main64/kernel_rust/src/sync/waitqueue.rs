//! Scheduler-agnostic wait-queue primitives.
//!
//! This module only tracks waiter registration.  Blocking/unblocking tasks is
//! handled by adapter functions in `waitqueue_adapter.rs`.

use core::sync::atomic::{AtomicBool, Ordering};

/// Multi-waiter queue with wake-all semantics.
///
/// `N` is the maximum number of waiters tracked by this queue.
pub struct WaitQueue<const N: usize> {
    waiters: [AtomicBool; N],
}

impl<const N: usize> WaitQueue<N> {
    pub const fn new() -> Self {
        Self {
            waiters: [const { AtomicBool::new(false) }; N],
        }
    }

    /// Registers `task_id` as waiting.
    ///
    /// Returns `false` when `task_id` is out of range for this queue.
    pub fn register_waiter(&self, task_id: usize) -> bool {
        if task_id >= N {
            return false;
        }
        self.waiters[task_id].store(true, Ordering::Release);
        true
    }

    /// Clears the waiting flag for `task_id`.
    pub fn clear_waiter(&self, task_id: usize) {
        if task_id < N {
            self.waiters[task_id].store(false, Ordering::Release);
        }
    }

    /// Calls `wake(task_id)` for every waiter currently registered and clears
    /// all waiter flags.
    pub fn wake_all(&self, mut wake: impl FnMut(usize)) {
        for task_id in 0..N {
            if self.waiters[task_id].swap(false, Ordering::AcqRel) {
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
