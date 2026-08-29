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

/// Returns the packed task identifier of the currently running task, if any.
///
/// This is a *packed* task identifier (slot + generation), in the same format
/// returned by the spawn functions -- it is *not* a bare slot index. Callers
/// that need to compare it against a bare slot (for example a self-wait
/// check) must first extract the slot portion via `task_id_slot`.
pub fn current_task_id() -> Option<usize> {
    with_scheduler(|meta| {
        meta.running_slot
            .map(|slot| super::types::pack_task_id(slot, meta.slots[slot].generation))
    })
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

/// Returns whether `task_id` holds the privileged-syscall capability.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this query.
///
/// An unused/unknown slot is treated as unprivileged. This is the seed of a
/// capability model (M6, `docs/CODE_REVIEW_2026-07-23.md`): today it gates
/// only the `Shutdown` syscall.
pub fn is_task_privileged(task_id: usize) -> bool {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        slot < meta.slots.len() && meta.slots[slot].used && meta.slots[slot].privileged
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

/// Returns a mutable reference to the DriverCaps of the currently running task,
/// or `None` if the task holds no capabilities (normal unprivileged task).
pub fn current_task_caps() -> Option<&'static mut crate::process::capabilities::DriverCaps> {
    with_scheduler(|meta| {
        let slot = meta.running_slot?;
        let entry = meta.slots.get(slot)?;
        if entry.caps.is_null() {
            None
        } else {
            // SAFETY:
            // - `entry.caps` was heap-allocated with Box::into_raw and is non-null.
            // - Syscall dispatch occurs on a single-core kernel where the current task
            //   runs exclusively and no other thread or ISR aliases `entry.caps`.
            Some(unsafe { &mut *entry.caps })
        }
    })
}

/// Sets or replaces the driver capabilities block for `task_id`.
///
/// Returns `false` if `task_id` is invalid or refers to an unused slot.
pub fn set_task_caps(task_id: usize, caps: *mut crate::process::capabilities::DriverCaps) -> bool {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            return false;
        }

        meta.slots[slot].caps = caps;
        true
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

/// Attempts to record one more `Exec`-spawned child for `task_id`, enforcing
/// a per-task cap.
///
/// Returns `true` and increments the task's `exec_count` when the task is a
/// live slot whose current count is still below `max`. Returns `false`
/// (without mutating state) when the slot is unused/unknown or the cap has
/// already been reached.
///
/// Stopgap denial-of-service guard (M10, `docs/CODE_REVIEW_2026-07-26.md`):
/// `syscall_exec_impl` calls this before spawning a child so an unprivileged
/// ring-3 task cannot loop `Exec` to exhaust scheduler slots or PMM frames.
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this mutation.
pub fn try_increment_exec_count(task_id: usize, max: u32) -> bool {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            return false;
        }

        let entry = &mut meta.slots[slot];
        if entry.exec_count >= max {
            return false;
        }

        entry.exec_count += 1;
        true
    })
}

/// Maximum number of `(child, parent)` `Exec`-spawn relationships retained in
/// [`SchedulerMetadata::parent_log`](super::types::SchedulerMetadata) for
/// `is_parent_of` authorization checks.
///
/// Bounded so a sustained stream of `Exec` calls across many different tasks
/// (each individually capped by `MAX_CHILD_EXECS` in
/// `syscall::dispatch::process`) cannot grow scheduler memory without bound;
/// the oldest record is evicted once the cap is reached.
const PARENT_LOG_CAPACITY: usize = 256;

/// Records `parent_task_id` as the spawning parent of `task_id`.
///
/// Returns `false` (without mutating state) if `task_id`'s slot is unused.
///
/// Stopgap authorization mechanism for `Wait` (M10,
/// `docs/CODE_REVIEW_2026-07-26.md`): `syscall_exec_impl` calls this right
/// after a successful spawn so `is_parent_of` can later scope which tasks the
/// caller is authorized to `Wait` on. The record is appended to
/// `SchedulerMetadata::parent_log` rather than stored on `TaskEntry`, so it
/// remains queryable after the child exits and its slot is reaped (the
/// common case: a parent typically calls `Wait` after, or racing, the
/// child's exit).
///
/// `task_id` is a packed task identifier (slot + generation) as returned by
/// the spawn functions; the generation portion is ignored by this mutation.
/// `parent_task_id` is stored verbatim as the full packed identifier of the
/// caller at spawn time.
pub fn set_task_parent(task_id: usize, parent_task_id: usize) -> bool {
    let slot = task_id_slot(task_id);
    with_scheduler(|meta| {
        if slot >= meta.slots.len() || !meta.slots[slot].used {
            return false;
        }

        // Step 1: Evict the oldest record once the log is at capacity so it
        // cannot grow without bound (M10). `remove(0)` is O(n), but n is
        // capped at `PARENT_LOG_CAPACITY`, so this stays cheap.
        if meta.parent_log.len() >= PARENT_LOG_CAPACITY {
            meta.parent_log.remove(0);
        }

        // Step 2: Best-effort append. If the reservation fails, the spawn
        // itself already succeeded; simply not recording lineage here just
        // means `is_parent_of` later falls back to its safe default (deny),
        // rather than this call failing the already-completed spawn.
        if meta.parent_log.try_reserve(1).is_ok() {
            meta.parent_log.push((task_id, parent_task_id));
        }

        true
    })
}

/// Returns whether `parent_task_id` is a recorded `Exec`-spawning parent of
/// `child_task_id`.
///
/// Both arguments are compared as full packed task identifiers (slot +
/// generation), so a coincidental slot reuse cannot be mistaken for a live
/// parent/child relationship (R-18): a reused slot's current generation no
/// longer matches the stale `child_task_id` used to record the original
/// relationship.
///
/// Looks up `SchedulerMetadata::parent_log` rather than live `TaskEntry`
/// state, so this remains accurate even after `child_task_id` has already
/// exited and been reaped -- see `set_task_parent`.
pub fn is_parent_of(parent_task_id: usize, child_task_id: usize) -> bool {
    with_scheduler(|meta| {
        meta.parent_log
            .iter()
            .any(|&(child, parent)| child == child_task_id && parent == parent_task_id)
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

/// Sets the currently running task slot index for unit tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_running_slot_for_test(slot: Option<usize>) {
    with_scheduler(|meta| {
        meta.running_slot = slot;
    });
}
