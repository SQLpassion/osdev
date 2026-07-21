//! State inspection, query, and diagnostics APIs for the scheduler.

use super::types::{task_id_slot, TaskState};
use super::with_scheduler;
use crate::arch::interrupts::{InterruptStackFrame, SavedRegisters};
use crate::memory::vmm;
use core::mem::size_of;

/// Returns the saved frame pointer for `task_id` if that slot is active.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this query.
///
/// Primarily intended for integration tests and diagnostics.
pub fn task_frame_ptr(task_id: usize) -> Option<*mut SavedRegisters> {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            None
        } else {
            Some(meta.slots[slot].frame_ptr)
        }
    })
}

/// Returns a copy of the initial interrupt return frame for `task_id`.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this query.
///
/// Intended for tests that validate kernel/user frame construction semantics.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_iret_frame(task_id: usize) -> Option<InterruptStackFrame> {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            return None;
        }
        let frame_ptr = meta.slots[slot].frame_ptr as usize;
        let iret_ptr = frame_ptr + size_of::<SavedRegisters>();
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `frame_ptr` belongs to the scheduler-owned stack for this task.
        // - `InterruptStackFrame` is written directly behind `SavedRegisters`.
        Some(unsafe { *(iret_ptr as *const InterruptStackFrame) })
    })
}

/// Returns the slot index of the currently running task, if any.
///
/// This is the raw slot index used internally by the scheduler.  It is *not*
/// a packed task identifier and should not be compared directly with values
/// returned by the spawn functions; use `task_id_slot` to extract the slot
/// portion of a packed identifier when necessary.
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
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this mutation.
///
/// The scheduler uses `kernel_rsp_top` to update `TSS.RSP0` before resuming
/// this task, so future ring3->ring0 transitions enter on the task-specific
/// kernel stack.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_task_user_context(task_id: usize, cr3: u64, user_rsp: u64, kernel_rsp_top: u64) -> bool {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            return false;
        }

        let entry = &mut meta.slots[slot];
        entry.cr3 = cr3;
        entry.user_rsp = user_rsp;
        entry.user_heap_top = vmm::USER_HEAP_BASE;
        entry.kernel_rsp_top = kernel_rsp_top;
        entry.is_user = true;
        true
    })
}

/// Returns whether `task_id` is configured as a user-mode task.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this query.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_user_task(task_id: usize) -> bool {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        slot < meta.slots.len() && meta.slots[slot].used && meta.slots[slot].is_user
    })
}

/// Returns task context tuple `(cr3, user_rsp, kernel_rsp_top)` for `task_id`.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this query.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_context(task_id: usize) -> Option<(u64, u64, u64)> {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            None
        } else {
            let entry = &meta.slots[slot];
            Some((entry.cr3, entry.user_rsp, entry.kernel_rsp_top))
        }
    })
}

/// Returns the lifecycle state of `task_id`, or `None` if the slot is unused.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this query.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_state(task_id: usize) -> Option<TaskState> {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            None
        } else {
            Some(meta.slots[slot].state)
        }
    })
}

/// Returns the generation counter of `task_id`, or `None` if the slot is unused.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions.  The generation portion of the identifier is ignored
/// for the lookup; the returned value is the generation currently stored in
/// the slot, which is `0` for free slots.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_generation(task_id: usize) -> Option<u64> {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            None
        } else {
            Some(meta.slots[slot].generation)
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
