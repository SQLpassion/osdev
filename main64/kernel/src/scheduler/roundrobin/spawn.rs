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
///
/// `privileged` grants the privileged-syscall capability (currently gating
/// `Shutdown`, see M6 in `docs/CODE_REVIEW_2026-07-23.md`). Callers should
/// pass `false` unless spawning a task that legitimately needs that
/// capability (today, only the boot shell).
pub fn spawn_user_task(
    entry_rip: u64,
    user_rsp: u64,
    cr3: u64,
    privileged: bool,
) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::User {
        entry_rip,
        user_rsp,
        cr3,
        privileged,
    })
}

/// Creates a new user task that owns dedicated user-code pages.
///
/// Use this for loader-backed binaries that were copied into private PMM
/// frames.
///
/// Historically this differed from [`spawn_user_task`] in whether task
/// teardown released the mapped user-code PFNs back to PMM (a manual
/// `release_user_code_pfns` boolean policy). That policy was replaced by
/// PMM-level frame refcounting (see `memory::pmm::manager::inc_refcount` /
/// `release_pfn`): teardown now always calls the refcounted `release_pfn` for
/// every mapped code leaf, and a frame that is genuinely shared with another
/// mapping (bumped via `inc_refcount` at the site that created the alias)
/// simply survives until its other owner releases it too. Both spawn
/// functions are therefore equivalent today; both are kept as named entry
/// points for call-site clarity (loader-owned vs. ad-hoc/test task images).
///
/// `privileged` grants the privileged-syscall capability (currently gating
/// `Shutdown`, see M6 in `docs/CODE_REVIEW_2026-07-23.md`). Callers should
/// pass `false` unless spawning a task that legitimately needs that
/// capability (today, only the boot shell).
pub fn spawn_user_task_owning_code(
    entry_rip: u64,
    user_rsp: u64,
    cr3: u64,
    privileged: bool,
) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::User {
        entry_rip,
        user_rsp,
        cr3,
        privileged,
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

        let (frame_ptr, cr3, user_rsp, kernel_rsp_top, is_user, privileged) = match kind {
            SpawnKind::Kernel { entry } => {
                let (frame_ptr, kernel_rsp_top) =
                    build_initial_kernel_task_frame(stack_ptr, TASK_STACK_SIZE, entry);
                // Kernel tasks never cross the syscall boundary (they call kernel
                // functions directly), so the privileged flag is inert for them.
                (frame_ptr, 0, 0, kernel_rsp_top, false, false)
            }
            SpawnKind::User {
                entry_rip,
                user_rsp,
                cr3,
                privileged,
            } => {
                let (frame_ptr, kernel_rsp_top) =
                    build_initial_user_task_frame(stack_ptr, TASK_STACK_SIZE, entry_rip, user_rsp);
                (frame_ptr, cr3, user_rsp, kernel_rsp_top, true, privileged)
            }
        };

        // Step 1: Acquire a fresh generation for this task under the scheduler
        // lock.  Fetching here (rather than before the lock) keeps the atomic
        // counter tightly coupled with slot allocation and avoids burning a
        // generation if the later frame construction fails.
        let generation = NEXT_TASK_GENERATION.fetch_add(1, AtomicOrdering::Relaxed) as u32 as u64;

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
            privileged,
            // The Exec child-rate-limit counter always starts at zero for a
            // freshly (re)used slot (M10); parent/child lineage for `Wait`
            // authorization is tracked separately in
            // `SchedulerMetadata::parent_log`, not on `TaskEntry`, so it
            // survives this task's eventual reap (see `set_task_parent`).
            exec_count: 0,
            stack_base: stack_ptr,
            stack_size: TASK_STACK_SIZE,
            fpu_state: fpu_ptr,
            caps: core::ptr::null_mut(),
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
