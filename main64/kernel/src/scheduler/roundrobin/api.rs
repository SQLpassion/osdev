//! State inspection, query, and diagnostics APIs for the scheduler.

use core::mem::size_of;
use crate::arch::interrupts::{InterruptStackFrame, SavedRegisters};
use crate::memory::vmm;
use super::types::TaskState;
use super::with_scheduler;

/// Returns the saved frame pointer for `task_id` if that slot is active.
///
/// Primarily intended for integration tests and diagnostics.
pub fn task_frame_ptr(task_id: usize) -> Option<*mut SavedRegisters> {
    with_scheduler(|meta| {
        if task_id >= meta.slots.len() || !meta.slots[task_id].used {
            None
        } else {
            Some(meta.slots[task_id].frame_ptr)
        }
    })
}

/// Returns a copy of the initial interrupt return frame for `task_id`.
///
/// Intended for tests that validate kernel/user frame construction semantics.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_iret_frame(task_id: usize) -> Option<InterruptStackFrame> {
    with_scheduler(|meta| {
        if task_id >= meta.slots.len() || !meta.slots[task_id].used {
            return None;
        }
        let frame_ptr = meta.slots[task_id].frame_ptr as usize;
        let iret_ptr = frame_ptr + size_of::<SavedRegisters>();
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `frame_ptr` belongs to the scheduler-owned stack for this task.
        // - `InterruptStackFrame` is written directly behind `SavedRegisters`.
        Some(unsafe { *(iret_ptr as *const InterruptStackFrame) })
    })
}

/// Returns the slot index of the currently running task, if any.
pub fn current_task_id() -> Option<usize> {
    with_scheduler(|meta| meta.running_slot)
}

/// Returns the current length of the internal slot table.
///
/// After every task removal `remove_task` trims trailing unused entries, so
/// this value reflects the number of slots up to and including the last live
/// task. It shrinks when the highest-index tasks exit and grows when new tasks
/// are spawned beyond the current length.
///
/// Trade-off (explicit):
/// - This is not equal to "number of live tasks" when interior holes exist.
/// - `slots` is a high-water-mark table with hole reuse, not a compact vector.
///
/// Primarily intended for integration tests that verify the Vec-shrink contract.
#[cfg_attr(not(test), allow(dead_code))]
pub fn slot_table_len() -> usize {
    with_scheduler(|meta| meta.slots.len())
}

/// Marks an existing task as user-mode task context.
///
/// The scheduler uses `kernel_rsp_top` to update `TSS.RSP0` before resuming
/// this task, so future ring3->ring0 transitions enter on the task-specific
/// kernel stack.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_task_user_context(task_id: usize, cr3: u64, user_rsp: u64, kernel_rsp_top: u64) -> bool {
    with_scheduler(|meta| {
        if task_id >= meta.slots.len() || !meta.slots[task_id].used {
            return false;
        }

        let slot = &mut meta.slots[task_id];
        slot.cr3 = cr3;
        slot.user_rsp = user_rsp;
        slot.user_heap_top = vmm::USER_HEAP_BASE;
        slot.kernel_rsp_top = kernel_rsp_top;
        slot.is_user = true;
        true
    })
}

/// Returns whether `task_id` is configured as a user-mode task.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_user_task(task_id: usize) -> bool {
    with_scheduler(|meta| {
        task_id < meta.slots.len() && meta.slots[task_id].used && meta.slots[task_id].is_user
    })
}

/// Returns task context tuple `(cr3, user_rsp, kernel_rsp_top)` for `task_id`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_context(task_id: usize) -> Option<(u64, u64, u64)> {
    with_scheduler(|meta| {
        if task_id >= meta.slots.len() || !meta.slots[task_id].used {
            None
        } else {
            let slot = &meta.slots[task_id];
            Some((slot.cr3, slot.user_rsp, slot.kernel_rsp_top))
        }
    })
}

/// Returns the lifecycle state of `task_id`, or `None` if the slot is unused.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_state(task_id: usize) -> Option<TaskState> {
    with_scheduler(|meta| {
        if task_id >= meta.slots.len() || !meta.slots[task_id].used {
            None
        } else {
            Some(meta.slots[task_id].state)
        }
    })
}

/// Gets the user heap top address of the current task.
///
/// Returns `None` if there is no current task or it is not a user task.
pub fn current_user_heap_top() -> Option<u64> {
    // Step 1: Acquire scheduler lock to safely inspect current task metadata.
    with_scheduler(|meta| {
        // Step 2: Resolve the slot ID of the currently selected/running task.
        let slot = meta.running_slot?;
        let entry = meta.slots.get(slot)?;

        // Step 3: Only user tasks carry a valid user heap boundary; return it.
        if entry.is_user {
            Some(entry.user_heap_top)
        } else {
            None
        }
    })
}

/// Sets the user heap top address of the current task.
///
/// Returns `false` if there is no current task or it is not a user task.
pub fn set_current_user_heap_top(new_top: u64) -> bool {
    // Step 1: Acquire scheduler lock to mutate the current task's state.
    with_scheduler(|meta| {
        // Step 2: Resolve the slot ID of the currently selected/running task.
        if let Some(slot) = meta.running_slot {
            if let Some(entry) = meta.slots.get_mut(slot) {
                // Step 3: Mutate only if the task runs user context.
                if entry.is_user {
                    entry.user_heap_top = new_top;
                    return true;
                }
            }
        }
        false
    })
}

/// Resets the scheduler initialization state to `false`.
///
/// This is a test-only helper to simulate initialization failure.
#[cfg_attr(not(test), allow(dead_code))]
pub fn reset_initialization_for_test() {
    // Step 1: Acquire scheduler lock to safely modify initialization metadata.
    with_scheduler(|meta| {
        // Step 2: Clear initialized state.
        meta.initialized = false;
    });
}
