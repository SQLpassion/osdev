use core::sync::atomic::{AtomicBool, Ordering};

use crate::scheduler;
use crate::sync::waitqueue::WaitQueue;
use crate::sync::waitqueue_adapter;

/// A synchronization primitive for managing a single active in-flight request.
///
/// It provides exclusive ownership of a request slot. In a scheduler context,
/// it blocks cooperatively on a wait queue, allowing other tasks to run. In
/// early-boot or test contexts without a scheduler, it spin-waits.
pub struct InFlightSlot {
    in_flight: AtomicBool,
    waitqueue: WaitQueue,
}

impl InFlightSlot {
    /// Creates a new `InFlightSlot`.
    pub const fn new() -> Self {
        Self {
            in_flight: AtomicBool::new(false),
            waitqueue: WaitQueue::new(),
        }
    }

    /// Acquires exclusive ownership of the request slot.
    pub fn acquire(&self) -> SingleSlotGuard<'_> {
        loop {
            // Step 1: fast path — try to claim exclusive ownership.
            if let Some(guard) = self.try_acquire() {
                return guard;
            }

            // Step 2: decide whether cooperative sleeping is possible in this
            // execution context (scheduler running + current task available).
            let maybe_task_id = if scheduler::is_running() {
                scheduler::current_task_id()
            } else {
                None
            };

            if let Some(task_id) = maybe_task_id {
                // Step 3a: scheduler context — sleep while the request slot stays busy.
                // Predicate is rechecked with interrupts disabled by waitqueue adapter.
                if waitqueue_adapter::sleep_if_multi(&self.waitqueue, task_id, || {
                    self.in_flight.load(Ordering::Acquire)
                })
                .should_yield()
                {
                    // Hand CPU to current owner or another runnable task.
                    scheduler::yield_now();
                }
            } else {
                // Step 3b: early boot/test context — no scheduler sleep available.
                core::hint::spin_loop();
            }
        }
    }

    /// Attempts to acquire exclusive ownership of the request slot without blocking.
    /// Returns `Some(SingleSlotGuard)` if successful, `None` otherwise.
    pub fn try_acquire(&self) -> Option<SingleSlotGuard<'_>> {
        if self
            .in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(SingleSlotGuard { slot: self })
        } else {
            None
        }
    }
}

impl Default for InFlightSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII token for exclusive ownership of an `InFlightSlot`.
pub struct SingleSlotGuard<'a> {
    slot: &'a InFlightSlot,
}

impl<'a> Drop for SingleSlotGuard<'a> {
    fn drop(&mut self) {
        // Step 1: release the in-flight marker so a waiting request can claim it.
        self.slot.in_flight.store(false, Ordering::Release);

        // Step 2: wake request waiters outside of any lock so blocked tasks
        // can re-contend for the request slot.
        waitqueue_adapter::wake_all_multi(&self.slot.waitqueue);
    }
}
