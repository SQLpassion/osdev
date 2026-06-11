//! Core scheduling state mutations and task management logic.

use core::mem::size_of;
use core::ptr;

extern crate alloc;
use alloc::vec::Vec;

use crate::arch::fpu;
use crate::arch::interrupts::{InterruptStackFrame, SavedRegisters};
use crate::memory::vmm;
use super::context::free_task_stack;
use super::types::{SchedulerArchCallbacks, SchedulerMetadata, TaskEntry, TaskState};
use super::{active_cr3_value, kernel_cr3_value, set_active_cr3};

/// Resolves a trap frame pointer back to its owning task slot.
pub(crate) fn find_entry_by_frame(
    meta: &SchedulerMetadata,
    frame_ptr: *const SavedRegisters,
) -> Option<usize> {
    if frame_ptr.is_null() {
        return None;
    }

    meta.run_queue
        .iter()
        .find(|&&slot| meta.slots[slot].used && meta.slots[slot].is_frame_within_stack(frame_ptr))
        .copied()
}

/// Returns `true` when `frame_ptr` lies within any scheduler-owned task stack,
/// including stacks from recently terminated tasks that are pending deallocation.
pub(crate) fn frame_within_any_task_stack(
    meta: &SchedulerMetadata,
    frame_ptr: *const SavedRegisters,
) -> bool {
    if frame_ptr.is_null() {
        return false;
    }

    // Check active task slots.
    for slot in meta.slots.iter() {
        if slot.used && slot.is_frame_within_stack(frame_ptr) {
            return true;
        }
    }

    // Check stacks from recently terminated tasks that haven't been freed yet.
    let frame_start = frame_ptr as usize;
    let frame_end = frame_start + size_of::<SavedRegisters>() + size_of::<InterruptStackFrame>();
    for &(base, size) in meta.pending_free_stacks.iter() {
        if !base.is_null() {
            let stack_start = base as usize;
            let stack_end = stack_start + size;
            if frame_start >= stack_start && frame_end <= stack_end {
                return true;
            }
        }
    }

    false
}

/// Removes `task_id` from the run queue and clears its slot.
///
/// The task's stack is added to the pending-free list in `meta` so that
/// `frame_within_any_task_stack` can still detect stale frame pointers.
/// Actual deallocation happens on the next `on_timer_tick` or in `init`.
///
/// Returns `true` when an active task was removed.
pub(crate) fn remove_task(
    meta: &mut SchedulerMetadata,
    task_id: usize,
    callbacks: SchedulerArchCallbacks,
) -> bool {
    // Step 1: reject invalid or already-free task IDs.
    if task_id >= meta.slots.len() || !meta.slots[task_id].used {
        return false;
    }

    // Step 2: locate the task inside the compact run queue.
    // If it is not present, scheduler metadata is inconsistent for this ID.
    let Some(removed_pos) = meta.run_queue.iter().position(|&s| s == task_id) else {
        return false;
    };

    // Step 3: precompute address-space teardown intent.
    // Kernel tasks carry no user CR3, so no address-space cleanup is needed.
    let mut cleanup = if meta.slots[task_id].is_user {
        Some((
            meta.slots[task_id].cr3,
            meta.slots[task_id].release_user_code_pfns,
        ))
    } else {
        None
    };

    if let Some((cr3, _)) = cleanup {
        if active_cr3_value() == cr3 {
            let kernel_cr3 = kernel_cr3_value();

            if kernel_cr3 != 0 && kernel_cr3 != cr3 {
                // Before destroying a user address space, ensure we are not
                // still executing with that CR3 active on this CPU.
                // SAFETY:
                // - This requires `unsafe` because changing CPU address-space state is a privileged operation outside Rust's guarantees.
                // - `kernel_cr3` is configured by scheduler owner via
                //   `set_kernel_address_space_cr3`.
                // - Switching away avoids releasing an address space that is
                //   currently active in CR3.
                unsafe {
                    (callbacks.switch_cr3)(kernel_cr3);
                }

                set_active_cr3(kernel_cr3);
            } else {
                // This branch indicates a scheduler bug:
                // - kernel_cr3 == cr3: a user task was spawned with the kernel CR3,
                //   which violates the address-space isolation invariant.
                //
                // In either case there is no safe CR3 to switch to before teardown.
                // Destroying the user address space while it is the active CR3 would
                // release the PML4 frame and leave the CPU pointing at freed memory,
                // causing an immediate triple fault. Skipping cleanup leaks the
                // address space, but avoids a harder crash.
                //
                // Slot and stack cleanup still proceed below so scheduler state
                // does not become permanently inconsistent.
                debug_assert!(
                    false,
                    "remove_task: cannot tear down user CR3 {:#x} — \
                     no safe kernel CR3 available (kernel_cr3={:#x}).",
                    cr3, kernel_cr3,
                );

                cleanup = None;
            }
        }
    }

    // Free the FPU state buffer immediately.
    // Unlike the task stack, the FPU buffer is not an execution stack — the CPU
    // is never "running on" it — so it is always safe to free here, even from
    // inside an IRQ handler.  Nesting the heap spinlock inside the scheduler
    // spinlock is safe on this single-core kernel because the scheduler lock
    // already has interrupts disabled (CLI).
    //
    // Clear fpu_owner if this task was the FPU owner so select_next_task does
    // not try to FXSAVE into a freed buffer.
    if meta.fpu_owner == Some(task_id) {
        meta.fpu_owner = None;
    }
    // SAFETY:
    // - This requires `unsafe` because it frees a raw heap allocation.
    // - `fpu_state` was returned by `fpu::FpuState::allocate_default` and is
    //   not used after this call (the slot is cleared below).
    unsafe {
        fpu::FpuState::deallocate(meta.slots[task_id].fpu_state);
    }

    meta.slots[task_id].fpu_state = ptr::null_mut();

    // Move the stack to the pending-free list instead of freeing it now.
    // This keeps the stack range visible to `frame_within_any_task_stack`
    // until the next timer tick, preventing stale task frames from being
    // mistaken for bootstrap frames.
    //
    // Immediate stack freeing is NOT a safe fallback here: `terminate_task`
    // can be called while the caller is still executing on the target task's
    // stack. Freeing it immediately would leave the CPU on a freed allocation.
    //
    // `try_reserve(1)` is used because this path can run inside an IRQ handler
    // (via `reap_zombies`). On this single-core kernel the scheduler spinlock
    // already disabled interrupts (`cli`), so nesting the heap spinlock is
    // safe. OOM here would be a stack leak, but is structurally unreachable:
    // the list is drained to empty on every timer tick, so its length is
    // bounded by the number of tasks terminated since the previous tick.
    if !meta.slots[task_id].stack_base.is_null() && meta.pending_free_stacks.try_reserve(1).is_ok()
    {
        meta.pending_free_stacks.push((
            meta.slots[task_id].stack_base,
            meta.slots[task_id].stack_size,
        ));
    }

    // Step 4: compact the run queue by removing the entry at `removed_pos`.
    // `Vec::remove` performs the same O(n) left-shift as the previous manual loop.
    meta.run_queue.remove(removed_pos);

    // If the removed slot was marked as currently running, clear the marker.
    if meta.running_slot == Some(task_id) {
        meta.running_slot = None;
    }

    // Clear the slot after we copied all required metadata out of it.
    meta.slots[task_id] = TaskEntry::empty();

    // Step 5: keep round-robin cursor valid after compaction.
    // - queue empty: reset to 0
    // - removed before cursor: shift cursor one step left
    // - cursor now out-of-range: clamp to last remaining entry
    if meta.run_queue.is_empty() {
        meta.current_queue_pos = 0;
    } else if removed_pos < meta.current_queue_pos {
        meta.current_queue_pos -= 1;
    } else if meta.current_queue_pos >= meta.run_queue.len() {
        meta.current_queue_pos = meta.run_queue.len() - 1;
    }

    // Step 6: trim trailing unused slots to prevent unbounded Vec growth at
    // the tail.
    //
    // Only trailing entries can be removed safely: `run_queue` stores slot
    // indices, so removing from the middle or from an index that is still live
    // would invalidate those references. Unused entries at the tail (beyond the
    // last `used` slot) have no `run_queue` entry and no `running_slot` claim,
    // so truncating them is safe.
    //
    // Trade-off (explicit): the Vec may still retain interior holes forever
    // under churny ID patterns. These holes are reused by first-fit in
    // `spawn_internal`, but they do not reduce `slots.len()` until they are
    // part of the trailing unused suffix.
    let live_end = meta.slots.iter().rposition(|s| s.used).map_or(0, |i| i + 1);
    meta.slots.truncate(live_end);

    // Final step: release user address-space resources if cleanup is safe.
    if let Some((cr3, release_user_code_pfns)) = cleanup {
        vmm::destroy_user_address_space_with_options(cr3, release_user_code_pfns);
    }

    true
}

/// Switches CR3 to the selected task context.
pub(crate) fn apply_selected_address_space(
    meta: &mut SchedulerMetadata,
    selected_slot: usize,
    callbacks: SchedulerArchCallbacks,
) {
    let target_cr3 = if meta.slots[selected_slot].is_user {
        meta.slots[selected_slot].cr3
    } else {
        kernel_cr3_value()
    };

    debug_assert!(
        target_cr3 != 0,
        "scheduler selected task with invalid CR3 (slot={}, is_user={})",
        selected_slot,
        meta.slots[selected_slot].is_user
    );

    if target_cr3 == 0 || active_cr3_value() == target_cr3 {
        return;
    }

    // SAFETY:
    // - This requires `unsafe` because changing CPU address-space state is a privileged operation outside Rust's guarantees.
    // - `target_cr3` originates from scheduler-controlled task metadata.
    // - Backend callback defines platform-specific switch operation.
    unsafe {
        (callbacks.switch_cr3)(target_cr3);
    }

    set_active_cr3(target_cr3);
}

/// Removes all `Zombie` tasks from the run queue.
///
/// Called at the start of `on_timer_tick` — at that point execution has
/// already moved off the zombie's kernel stack (either onto a different
/// task's stack or onto the bootstrap stack), so freeing the slot is safe.
///
/// Zombie task stacks are moved to the pending-free list and will be
/// deallocated after releasing the scheduler lock.
pub(crate) fn reap_zombies(
    meta: &mut SchedulerMetadata,
    callbacks: SchedulerArchCallbacks,
) -> bool {
    let mut i = 0;
    let mut removed_any = false;

    while i < meta.run_queue.len() {
        let slot = meta.run_queue[i];

        if meta.slots[slot].state == TaskState::Zombie {
            // Step 1: Avoid reaping the currently running slot (e.g., during the Exit syscall).
            // Freeing the stack of the currently executing context while still running on it
            // causes memory corruption when the allocator overwrites the active stack frame.
            if meta.running_slot == Some(slot) {
                i += 1;
                continue;
            }

            if remove_task(meta, slot, callbacks) {
                removed_any = true;
            }
            // `remove_task` shifts entries down; re-check the same index.
            continue;
        }

        i += 1;
    }

    removed_any
}

/// Returns `bootstrap_frame` if set, otherwise falls back to `current_frame`.
///
/// Used in two places: when the task queue becomes empty and when the
/// debug/test stop hook asks to return to bootstrap context.
/// Centralising the fallback avoids repeating the same conditional.
#[inline]
pub(crate) fn bootstrap_or_current(
    meta: &SchedulerMetadata,
    current_frame: *mut SavedRegisters,
) -> *mut SavedRegisters {
    if !meta.bootstrap_frame.is_null() {
        meta.bootstrap_frame
    } else {
        current_frame
    }
}

/// Clears all volatile scheduling state after a full scheduler teardown.
///
/// Address-space configuration and `initialized` are preserved so the
/// scheduler can be restarted without re-registration.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn reset_scheduler_state(meta: &mut SchedulerMetadata) {
    meta.started = false;
    meta.bootstrap_frame = ptr::null_mut();
    meta.running_slot = None;
    meta.current_queue_pos = 0;
    meta.tick_count = 0;
    meta.run_queue.clear();
    meta.slots.clear();
    meta.pending_free_stacks.clear();
}

/// Selects the next runnable task in round-robin order and returns its frame.
///
/// Advances `current_queue_pos` and `running_slot`, programs TSS.RSP0 for user
/// tasks, and switches CR3 when address-space switching is enabled.
///
/// Falls back to `bootstrap_frame` (or `current_frame`) when all tasks are
/// blocked so the CPU can execute the idle `hlt` loop instead of spinning.
pub(crate) fn select_next_task(
    meta: &mut SchedulerMetadata,
    detected_slot: Option<usize>,
    current_frame: *mut SavedRegisters,
    callbacks: SchedulerArchCallbacks,
) -> *mut SavedRegisters {
    // Step 1: Close out the previous running mark before selecting the next slot.
    // Keep explicit non-running states (Blocked/Zombie) untouched.
    if let Some(previous_slot) = meta.running_slot {
        if previous_slot < meta.slots.len() && meta.slots[previous_slot].used {
            // Lazy FPU: if the outgoing task is the FPU owner, save its state.
            //
            // This must happen regardless of the task's scheduling state: a
            // task that blocked itself (e.g. a blocking syscall sets `Blocked`
            // before yielding) is no longer `Running` here, but its FPU/SSE
            // registers are still live in the CPU.  Skipping the save would
            // silently destroy the blocked task's FPU state — either when
            // another task triggers `#NM` and takes over ownership, or when
            // the blocked task resumes and `#NM` restores its stale buffer
            // over the (still correct) live registers.
            //
            // FXSAVE64 does not check CR0.TS, so it is safe to call here
            // regardless of the current CR0.TS value.  The CPU FPU registers
            // still contain the outgoing task's live state at this point
            // because hardware interrupts only save GPRs, not FPU registers.
            if meta.fpu_owner == Some(previous_slot) {
                let fpu_ptr = meta.slots[previous_slot].fpu_state;

                if !fpu_ptr.is_null() {
                    // SAFETY:
                    // - This requires `unsafe` because it executes privileged
                    //   inline assembly (FXSAVE64) and accesses a raw pointer.
                    // - `fpu_ptr` is a valid, 16-byte-aligned 512-byte buffer.
                    // - The outgoing task's FPU state is live in the CPU registers.
                    unsafe { (*fpu_ptr).save() };
                }

                meta.fpu_owner = None;
            }

            if meta.slots[previous_slot].state == TaskState::Running {
                meta.slots[previous_slot].state = TaskState::Ready;
            }
        }
    }

    let task_count = meta.run_queue.len();
    // Guard: a caller must ensure the run_queue is non-empty before calling
    // this function. An empty queue causes division-by-zero in the modulo
    // arithmetic below. `on_timer_tick` already enforces this via its
    // `run_queue.is_empty()` early-return, but the assert catches future
    // call sites that might miss the precondition.
    debug_assert!(
        task_count > 0,
        "select_next_task called with empty run_queue"
    );

    let base_pos = if let Some(slot) = detected_slot {
        meta.run_queue
            .iter()
            .position(|&s| s == slot)
            .unwrap_or(meta.current_queue_pos)
    } else {
        meta.current_queue_pos
    };

    let search_start_pos = (base_pos + 1) % task_count;

    let mut selected_pos = None;
    let mut selected_slot = 0usize;
    let mut selected_frame = ptr::null_mut();

    for step in 0..task_count {
        let pos = (search_start_pos + step) % task_count;
        let slot = meta.run_queue[pos];

        // Skip non-runnable tasks (blocked or zombie).
        if meta.slots[slot].state == TaskState::Blocked
            || meta.slots[slot].state == TaskState::Zombie
        {
            continue;
        }

        let frame = meta.slots[slot].frame_ptr;

        if meta.slots[slot].is_frame_within_stack(frame) {
            selected_pos = Some(pos);
            selected_slot = slot;
            selected_frame = frame;
            break;
        }
    }

    meta.tick_count = meta.tick_count.wrapping_add(1);

    // Set CR0.TS before switching to any context (task or bootstrap).
    // This ensures the next FPU/SSE instruction raises #NM so the lazy
    // switcher can restore the correct state.  FXSAVE64 (called above for
    // the outgoing owner) does not check CR0.TS and is unaffected by this.
    // SAFETY:
    // - This requires `unsafe` because it modifies a privileged control register.
    // - Valid only in ring 0; the scheduler always runs in ring 0.
    unsafe { fpu::set_ts() };

    if let Some(pos) = selected_pos {
        // Step 2: Persist scheduler-visible running state for the selected slot.
        meta.slots[selected_slot].state = TaskState::Running;
        meta.current_queue_pos = pos;
        meta.running_slot = Some(selected_slot);

        if meta.slots[selected_slot].is_user {
            (callbacks.set_kernel_rsp0)(meta.slots[selected_slot].kernel_rsp_top);
        }

        apply_selected_address_space(meta, selected_slot, callbacks);

        selected_frame
    } else {
        // All tasks are blocked — return to the idle loop so the CPU
        // can execute `hlt` instead of busy-spinning a blocked task.
        meta.running_slot = None;
        bootstrap_or_current(meta, current_frame)
    }
}

/// Drains `pending_free_stacks` for deallocation, re-queueing any stack that
/// still hosts `current_frame`.
///
/// `remove_task` defers stack frees precisely because the terminating call may
/// still be executing on the removed task's stack (e.g. `terminate_task` on
/// the caller's own ID).  If the next timer tick interrupts that execution,
/// the interrupted frame — and all code that runs until the IRQ stub finally
/// switches RSP via `mov rsp, rax` — still lives inside one of the pending
/// stacks.  Freeing that stack would hand it to the heap (free-list node and
/// header writes at the block base, possible coalescing of neighbors) while
/// the CPU is executing on it: a use-after-free.
///
/// Pending stacks never overlap, so at most one entry can contain
/// `current_frame`; a single `position` scan suffices.  The match is re-queued
/// for the next tick, by which point execution has provably moved off it (the
/// tick that re-queues it returns a different frame).  `try_reserve` keeps the
/// re-queue OOM-safe; on allocation failure the stack is intentionally leaked —
/// a bounded leak is strictly safer than freeing the active stack.
pub(crate) fn take_pending_stacks_for_free(
    meta: &mut SchedulerMetadata,
    current_frame: *const SavedRegisters,
) -> Vec<(*mut u8, usize)> {
    let mut stacks = core::mem::take(&mut meta.pending_free_stacks);

    let frame_addr = current_frame as usize;
    if let Some(index) = stacks.iter().position(|&(base, size)| {
        let start = base as usize;
        !base.is_null() && frame_addr >= start && frame_addr < start + size
    }) {
        let still_in_use = stacks.swap_remove(index);

        if meta.pending_free_stacks.try_reserve(1).is_ok() {
            meta.pending_free_stacks.push(still_in_use);
        }
    }

    stacks
}

/// Frees heap-allocated task stacks outside the scheduler lock.
pub(crate) fn free_pending_stacks(stacks: &[(*mut u8, usize)]) {
    for &(ptr, _size) in stacks {
        // SAFETY: Pointers were returned by `allocate_task_stack`.
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        unsafe {
            free_task_stack(ptr);
        }
    }
}
