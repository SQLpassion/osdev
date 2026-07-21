//! Task spawning implementation and validation logic.

use super::context::{
    allocate_task_stack, build_initial_kernel_task_frame, build_initial_user_task_frame,
    free_task_stack,
};
use super::types::{pack_task_id, SpawnError, SpawnKind, TaskEntry, TaskState};
use super::{with_scheduler, TASK_STACK_SIZE};
use crate::arch::fpu;
use crate::memory::vmm;
use core::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

/// Creates a new kernel task and appends it to the run queue.
///
/// Thin wrapper around the shared spawn path for kernel-mode tasks.
pub fn spawn_kernel_task(entry: extern "C" fn() -> !) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::Kernel { entry })
}

/// Creates a new user task with explicit user entry point and user stack pointer.
///
/// `entry_rip` and `user_rsp` are user-space virtual addresses in the task's
/// address space identified by `cr3`.
pub fn spawn_user_task(entry_rip: u64, user_rsp: u64, cr3: u64) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::User {
        entry_rip,
        user_rsp,
        cr3,
        release_user_code_pfns: false,
    })
}

/// Creates a new user task that owns dedicated user-code pages.
///
/// Use this for loader-backed binaries that were copied into private PMM
/// frames. On task teardown these code PFNs are released.
pub fn spawn_user_task_owning_code(
    entry_rip: u64,
    user_rsp: u64,
    cr3: u64,
) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::User {
        entry_rip,
        user_rsp,
        cr3,
        release_user_code_pfns: true,
    })
}

/// Global monotonic generation counter for task identity.
///
/// Generation `0` is reserved for "empty / invalidated" slots, so the first
/// spawned task receives generation `1`.  Each successful spawn atomically
/// increments this counter and stores the fetched value into the new
/// `TaskEntry`.  Together with the slot index this forms a unique task
/// identifier that survives slot reuse (R-18).
static NEXT_TASK_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Shared task creation path used by both public spawn wrappers.
///
/// The stack is heap-allocated *before* acquiring the scheduler lock to
/// avoid nested spinlock acquisition (scheduler lock + heap lock).
///
/// On success, returns a packed task identifier encoding both the slot index
/// and the task's generation.  Callers should treat this value as opaque and
/// pass it unchanged to scheduler APIs such as `wait_for_task_exit`.
fn spawn_internal(kind: SpawnKind) -> Result<usize, SpawnError> {
    // Pre-check: reject early if scheduler is not initialized,
    // before performing the (expensive) heap allocation.
    let pre_check = with_scheduler(|meta| {
        if !meta.initialized {
            return Err(SpawnError::NotInitialized);
        }
        Ok(())
    });

    pre_check?;

    // Allocate the stack and FPU state outside the scheduler lock to avoid
    // nesting the scheduler spinlock with the heap spinlock.
    let stack_ptr = allocate_task_stack();

    if stack_ptr.is_null() {
        return Err(SpawnError::StackAllocationFailed);
    }

    let fpu_ptr = fpu::FpuState::allocate_default();
    if fpu_ptr.is_null() {
        // SAFETY: `stack_ptr` was returned by `allocate_task_stack` and has
        // not been stored anywhere yet.
        unsafe { free_task_stack(stack_ptr) };
        return Err(SpawnError::StackAllocationFailed);
    }

    let result = with_scheduler(|meta| {
        // Re-check under lock — state may have changed between pre-check and now.
        if !meta.initialized {
            return Err(SpawnError::NotInitialized);
        }

        // Find a free (previously used) slot or determine that a new one must
        // be appended. `remove_task` trims trailing unused entries, so the Vec
        // length reflects the live high-water mark; new slots are pushed at the end.
        let (slot_idx, is_new_slot) = match meta.slots.iter().position(|s| !s.used) {
            Some(i) => (i, false),
            None => (meta.slots.len(), true),
        };

        // Pre-reserve Vec capacity so the actual push operations are infallible.
        // Both reservations happen before any state is mutated so that an OOM
        // during either reservation leaves the scheduler in a consistent state.
        if is_new_slot {
            meta.slots
                .try_reserve(1)
                .map_err(|_| SpawnError::StackAllocationFailed)?;
        }
        meta.run_queue
            .try_reserve(1)
            .map_err(|_| SpawnError::StackAllocationFailed)?;

        let (frame_ptr, cr3, user_rsp, kernel_rsp_top, is_user, release_user_code_pfns) = match kind
        {
            SpawnKind::Kernel { entry } => {
                let (frame_ptr, kernel_rsp_top) =
                    build_initial_kernel_task_frame(stack_ptr, TASK_STACK_SIZE, entry);
                (frame_ptr, 0, 0, kernel_rsp_top, false, false)
            }
            SpawnKind::User {
                entry_rip,
                user_rsp,
                cr3,
                release_user_code_pfns,
            } => {
                let (frame_ptr, kernel_rsp_top) =
                    build_initial_user_task_frame(stack_ptr, TASK_STACK_SIZE, entry_rip, user_rsp);
                (
                    frame_ptr,
                    cr3,
                    user_rsp,
                    kernel_rsp_top,
                    true,
                    release_user_code_pfns,
                )
            }
        };

        // Step 1: Acquire a fresh generation for this task under the scheduler
        // lock.  Fetching here (rather than before the lock) keeps the atomic
        // counter tightly coupled with slot allocation and avoids burning a
        // generation if the later frame construction fails.
        let generation = NEXT_TASK_GENERATION.fetch_add(1, AtomicOrdering::Relaxed);

        let entry = TaskEntry {
            used: true,
            state: TaskState::Ready,
            generation,
            frame_ptr,
            cr3,
            user_rsp,
            user_heap_top: if is_user { vmm::USER_HEAP_BASE } else { 0 },
            kernel_rsp_top,
            is_user,
            release_user_code_pfns,
            stack_base: stack_ptr,
            stack_size: TASK_STACK_SIZE,
            fpu_state: fpu_ptr,
        };

        if is_new_slot {
            meta.slots.push(entry); // capacity guaranteed by try_reserve above
        } else {
            meta.slots[slot_idx] = entry;
        }

        meta.run_queue.push(slot_idx); // capacity guaranteed by try_reserve above

        // Step 2: Return the opaque packed identifier.  The generation encoded
        // here is what waiters will use to detect slot reuse (R-18).
        Ok(pack_task_id(slot_idx, generation))
    });

    // If spawn failed after we already allocated the stack and FPU buffer, free them.
    if result.is_err() {
        // SAFETY:
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        // - `stack_ptr` and `fpu_ptr` were returned by their respective allocators
        //   and have not been stored in any task slot (spawn failed).
        unsafe {
            free_task_stack(stack_ptr);
            fpu::FpuState::deallocate(fpu_ptr);
        }
    }

    result
}
