//! Minimal kernel-mode round-robin scheduler.
//!
//! Phase 2 scope:
//! - static task pool (no heap allocations)
//! - timer-driven round-robin on IRQ0
//! - kernel-mode function pointers as task entries

use core::arch::asm;
use core::mem::size_of;
use core::ptr;

use crate::arch::gdt;
use crate::arch::interrupts::{self, InterruptStackFrame, SavedRegisters};
use crate::memory::vmm;
use crate::sync::spinlock::SpinLock;

/// Entry point type for schedulable kernel tasks.
///
/// Tasks are entered via a synthetic interrupt-return frame and are expected
/// to never return.
pub type KernelTaskFn = extern "C" fn() -> !;

const MAX_TASKS: usize = 8;
const TASK_STACK_SIZE: usize = 64 * 1024;
const PAGE_SIZE: usize = 4096;
const KERNEL_CODE_SELECTOR: u64 = gdt::KERNEL_CODE_SELECTOR as u64;
const KERNEL_DATA_SELECTOR: u64 = gdt::KERNEL_DATA_SELECTOR as u64;
const USER_CODE_SELECTOR: u64 = gdt::USER_CODE_SELECTOR as u64;
const USER_DATA_SELECTOR: u64 = gdt::USER_DATA_SELECTOR as u64;
const DEFAULT_RFLAGS: u64 = 0x202;

/// Internal task-construction descriptor for the shared spawn path.
///
/// Public APIs `spawn_kernel_task` and `spawn_user_task` are thin wrappers
/// that translate their parameters into one of these variants and call
/// `spawn_internal`.
enum SpawnKind {
    /// Kernel-mode task entered via function pointer.
    Kernel {
        /// Kernel entry function (`extern "C" fn() -> !`).
        entry: KernelTaskFn,
    },
    /// User-mode task entered via synthetic IRET frame.
    User {
        /// Initial user RIP to be placed into the IRET frame.
        entry_rip: u64,
        /// Initial user RSP to be placed into the IRET frame.
        user_rsp: u64,
        /// Address-space root (CR3 physical address) associated with task.
        cr3: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// Scheduler has not been initialized via [`init`].
    NotInitialized,

    /// Static task pool is full.
    CapacityExceeded,
}

/// Lifecycle state of a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TaskState {
    /// Task is eligible for scheduling.
    Ready,

    /// Task is the one currently executing on the CPU.
    Running,

    /// Task is waiting for an external event (e.g. keyboard input).
    Blocked,

    /// Task has exited but its slot and stack are still reserved.
    ///
    /// A zombie is never selected by the round-robin loop.  The scheduler
    /// reaps zombie slots at the beginning of the next [`on_timer_tick`]
    /// call, when execution is guaranteed to have moved off the zombie's
    /// kernel stack.
    ///
    /// This two-phase cleanup avoids a use-after-free race: if
    /// `exit_current_task` freed the slot immediately, a timer IRQ between
    /// the free and the subsequent `yield_now` could allow `spawn_*` to
    /// reuse the slot — overwriting the stack that the exiting code path
    /// is still running on.
    Zombie,
}

/// One slot in the static task table.
#[derive(Clone, Copy)]
struct TaskEntry {
    /// Slot allocation flag in the static task pool.
    /// `true` means the entry is currently owned by a live task.
    used: bool,

    /// Scheduler lifecycle state used by round-robin selection.
    /// Blocked tasks are skipped until explicitly unblocked.
    state: TaskState,

    /// Pointer to the currently saved register frame for this task.
    /// This is the resume target returned to the IRQ trampoline.
    frame_ptr: *mut SavedRegisters,

    /// Task address space root (future user-mode CR3 switch support).
    /// Kernel-only tasks currently keep this at zero.
    #[allow(dead_code)]
    cr3: u64,

    /// User-mode stack pointer for ring-3 resume (future user-task entry).
    /// Kernel-only tasks currently keep this at zero.
    #[allow(dead_code)]
    user_rsp: u64,

    /// Top of this task's kernel stack, used to program `TSS.RSP0`
    /// before resuming a user-mode task.
    kernel_rsp_top: u64,

    /// Marks whether this task should be treated as user-mode context.
    /// When set, scheduler updates `TSS.RSP0` from `kernel_rsp_top`.
    is_user: bool,

}

impl TaskEntry {
    /// Returns an unused slot marker.
    const fn empty() -> Self {
        Self {
            used: false,
            state: TaskState::Ready,
            frame_ptr: ptr::null_mut(),
            cr3: 0,
            user_rsp: 0,
            kernel_rsp_top: 0,
            is_user: false,
        }
    }

    /// Checks whether `frame_ptr` lies within the stack memory of `slot_idx`.
    fn is_frame_within_stack(
        &self,
        stacks: &[[u8; TASK_STACK_SIZE]; MAX_TASKS],
        slot_idx: usize,
        frame_ptr: *const SavedRegisters,
    ) -> bool {
        if frame_ptr.is_null() {
            return false;
        }
        let frame_start = frame_ptr as usize;
        let frame_end =
            frame_start + size_of::<SavedRegisters>() + size_of::<InterruptStackFrame>();

        let stack = &stacks[slot_idx];
        let stack_start = stack.as_ptr() as usize;
        let stack_end = stack_start + TASK_STACK_SIZE;
        frame_start >= stack_start && frame_end <= stack_end
    }
}

/// Runtime metadata of the round-robin scheduler.
struct SchedulerMetadata {
    /// Global initialization latch set by [`init`].
    /// Guards API usage before scheduler data structures are ready.
    initialized: bool,

    /// Indicates whether timer ticks should perform scheduling decisions.
    /// Set by [`start`], cleared on stop paths.
    started: bool,

    /// Cooperative stop request flag consumed by [`on_timer_tick`].
    /// When observed, scheduler returns to bootstrap frame and resets state.
    stop_requested: bool,

    /// Last non-task interrupt frame pointer (typically bootstrap/idle context).
    /// Used as fallback return frame when no runnable tasks exist.
    bootstrap_frame: *mut SavedRegisters,

    /// Slot index of currently selected/running task, if any.
    /// `None` when executing bootstrap/idle context.
    running_slot: Option<usize>,

    /// Cursor into `run_queue` used for round-robin progression.
    /// Points at the most recently selected queue position.
    current_queue_pos: usize,

    /// Number of active entries in `run_queue` and `slots`.
    task_count: usize,

    /// Compact queue of active task slot IDs in scheduling order.
    /// Only indices `< task_count` are valid.
    run_queue: [usize; MAX_TASKS],

    /// Per-slot task metadata table.
    /// `used=false` marks free slots.
    slots: [TaskEntry; MAX_TASKS],

    /// Total number of timer ticks processed while scheduler is started.
    /// Primarily for diagnostics/tests.
    tick_count: u64,

    /// Enables CR3 switching based on task type/context.
    /// Disabled by default for compatibility with early bring-up/tests.
    address_space_switching_enabled: bool,

    /// Physical PML4 address of kernel address space.
    /// Used when switching from user task back to kernel context.
    kernel_cr3: u64,

    /// Last CR3 value written by scheduler-managed switch path.
    /// Avoids redundant `mov cr3` on consecutive selections in same address space.
    active_cr3: u64,
}

impl SchedulerMetadata {
    /// Returns the initial scheduler metadata.
    const fn new() -> Self {
        Self {
            initialized: false,
            started: false,
            stop_requested: false,
            bootstrap_frame: ptr::null_mut(),
            running_slot: None,
            current_queue_pos: 0,
            task_count: 0,
            run_queue: [0; MAX_TASKS],
            slots: [TaskEntry::empty(); MAX_TASKS],
            tick_count: 0,
            address_space_switching_enabled: false,
            kernel_cr3: 0,
            active_cr3: 0,
        }
    }
}

/// Complete scheduler payload containing metadata and per-task stacks.
struct SchedulerData {
    meta: SchedulerMetadata,
    stacks: [[u8; TASK_STACK_SIZE]; MAX_TASKS],
}

impl SchedulerData {
    /// Creates zero-initialized scheduler storage.
    const fn new() -> Self {
        Self {
            meta: SchedulerMetadata::new(),
            stacks: [[0; TASK_STACK_SIZE]; MAX_TASKS],
        }
    }
}

unsafe impl Send for SchedulerData {}

// SAFETY:
// - `SchedulerData` is only accessed behind `SpinLock<SchedulerData>`.
// - Raw pointers in `meta` point into scheduler-owned stacks and are only
//   read/written while holding the lock.
static SCHED: SpinLock<SchedulerData> = SpinLock::new(SchedulerData::new());

/// Aligns `value` down to the given power-of-two `align`.
#[inline]
const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

/// Executes `f` while holding the scheduler spinlock.
#[inline]
fn with_sched<R>(f: impl FnOnce(&mut SchedulerData) -> R) -> R {
    let mut sched = SCHED.lock();
    f(&mut sched)
}

extern "C" fn task_return_trap() -> ! {
    exit_current_task()
}

/// Builds the initial kernel-task context on the stack of `slot_idx`.
///
/// Returns a pointer to the saved [`SavedRegisters`] used as scheduler context.
fn build_initial_kernel_task_frame(
    stacks: &mut [[u8; TASK_STACK_SIZE]; MAX_TASKS],
    slot_idx: usize,
    entry: KernelTaskFn,
) -> (*mut SavedRegisters, u64) {
    // SAFETY:
    // - `slot_idx` is validated by caller to be in-bounds and unique.
    // - Each slot owns a disjoint stack region in `stacks`.
    unsafe {
        let stack = &mut stacks[slot_idx];
        let stack_base = stack.as_mut_ptr() as usize;
        let stack_top = stack_base + TASK_STACK_SIZE;

        // Touch every stack page before first context switch.
        // This forces demand paging (if any) during spawn-time instead of in IRQ context.
        for page_off in (0..TASK_STACK_SIZE).step_by(PAGE_SIZE) {
            ptr::write_volatile(stack.as_mut_ptr().add(page_off), 0);
        }

        // SysV-friendly entry stack alignment.
        // Keep one return-address slot below RSP for a synthetic trap target.
        let entry_rsp = align_down(stack_top, 16) - 8;
        let iret_addr = entry_rsp - size_of::<InterruptStackFrame>();
        let frame_addr = iret_addr - size_of::<SavedRegisters>();

        let frame_ptr = frame_addr as *mut SavedRegisters;
        let iret_ptr = iret_addr as *mut InterruptStackFrame;

        // SAFETY:
        // - `entry_rsp` lies within the task's private stack memory.
        // - Writing a synthetic return address ensures an accidental task return
        //   traps into scheduler-controlled termination.
        ptr::write(
            entry_rsp as *mut u64,
            task_return_trap as *const () as usize as u64,
        );

        ptr::write(frame_ptr, SavedRegisters::default());
        ptr::write(
            iret_ptr,
            InterruptStackFrame {
                rip: entry as usize as u64,
                cs: KERNEL_CODE_SELECTOR,
                rflags: DEFAULT_RFLAGS,
                rsp: entry_rsp as u64,
                ss: KERNEL_DATA_SELECTOR,
            },
        );

        (frame_ptr, stack_top as u64)
    }
}

/// Builds an initial user-mode task context on the stack of `slot_idx`.
///
/// The saved interrupt frame is configured so that the next scheduler-selected
/// `iretq` transitions to ring 3 at `entry_rip` with user stack `user_rsp`.
fn build_initial_user_task_frame(
    stacks: &mut [[u8; TASK_STACK_SIZE]; MAX_TASKS],
    slot_idx: usize,
    entry_rip: u64,
    user_rsp: u64,
) -> (*mut SavedRegisters, u64) {
    // SAFETY:
    // - `slot_idx` is validated by caller to be in-bounds and unique.
    // - Each slot owns a disjoint stack region in `stacks`.
    unsafe {
        let stack = &mut stacks[slot_idx];
        let stack_base = stack.as_mut_ptr() as usize;
        let stack_top = stack_base + TASK_STACK_SIZE;

        for page_off in (0..TASK_STACK_SIZE).step_by(PAGE_SIZE) {
            ptr::write_volatile(stack.as_mut_ptr().add(page_off), 0);
        }

        let frame_addr = align_down(stack_top, 16)
            - size_of::<SavedRegisters>()
            - size_of::<InterruptStackFrame>();
        let frame_ptr = frame_addr as *mut SavedRegisters;
        let iret_ptr = (frame_addr + size_of::<SavedRegisters>()) as *mut InterruptStackFrame;

        ptr::write(frame_ptr, SavedRegisters::default());
        ptr::write(
            iret_ptr,
            InterruptStackFrame {
                rip: entry_rip,
                cs: USER_CODE_SELECTOR,
                rflags: DEFAULT_RFLAGS, // IF=1 so timer preemption remains active in user mode.
                rsp: user_rsp,
                ss: USER_DATA_SELECTOR,
            },
        );

        (frame_ptr, stack_top as u64)
    }
}

/// Resolves a trap frame pointer back to its owning task slot.
fn find_entry_by_frame(
    meta: &SchedulerMetadata,
    stacks: &[[u8; TASK_STACK_SIZE]; MAX_TASKS],
    frame_ptr: *const SavedRegisters,
) -> Option<usize> {
    if frame_ptr.is_null() {
        return None;
    }

    for pos in 0..meta.task_count {
        let slot = meta.run_queue[pos];
        if meta.slots[slot].used && meta.slots[slot].is_frame_within_stack(stacks, slot, frame_ptr)
        {
            return Some(slot);
        }
    }

    None
}

/// Returns `true` when `frame_ptr` lies within any scheduler-owned task stack.
fn frame_within_any_task_stack(
    stacks: &[[u8; TASK_STACK_SIZE]; MAX_TASKS],
    frame_ptr: *const SavedRegisters,
) -> bool {
    if frame_ptr.is_null() {
        return false;
    }

    let frame_start = frame_ptr as usize;
    let frame_end = frame_start + size_of::<SavedRegisters>() + size_of::<InterruptStackFrame>();

    for stack in stacks.iter() {
        let stack_start = stack.as_ptr() as usize;
        let stack_end = stack_start + TASK_STACK_SIZE;
        if frame_start >= stack_start && frame_end <= stack_end {
            return true;
        }
    }

    false
}

/// Removes `task_id` from the run queue and clears its slot.
///
/// Returns `true` when an active task was removed.
fn remove_task(meta: &mut SchedulerMetadata, task_id: usize) -> bool {
    if task_id >= MAX_TASKS || !meta.slots[task_id].used {
        return false;
    }

    let Some(removed_pos) = (0..meta.task_count).find(|pos| meta.run_queue[*pos] == task_id) else {
        return false;
    };

    let mut cleanup_cr3 = if meta.slots[task_id].is_user {
        Some(meta.slots[task_id].cr3)
    } else {
        None
    };

    if let Some(cr3) = cleanup_cr3 {
        if meta.active_cr3 == cr3 {
            if meta.kernel_cr3 != 0 && meta.kernel_cr3 != cr3 {
                // SAFETY:
                // - `kernel_cr3` is configured by scheduler owner via
                //   `set_kernel_address_space_cr3`.
                // - Switching away avoids releasing an address space that is
                //   currently active in CR3.
                unsafe {
                    vmm::switch_page_directory(meta.kernel_cr3);
                }
                meta.active_cr3 = meta.kernel_cr3;
            } else {
                // Without a known-safe fallback CR3, skip teardown to avoid
                // freeing the currently active address space.
                cleanup_cr3 = None;
            }
        }
    }

    for pos in removed_pos..meta.task_count - 1 {
        meta.run_queue[pos] = meta.run_queue[pos + 1];
    }
    meta.run_queue[meta.task_count - 1] = 0;
    meta.task_count -= 1;

    if meta.running_slot == Some(task_id) {
        meta.running_slot = None;
    }

    meta.slots[task_id] = TaskEntry::empty();

    if meta.task_count == 0 {
        meta.current_queue_pos = 0;
    } else if removed_pos < meta.current_queue_pos {
        meta.current_queue_pos -= 1;
    } else if meta.current_queue_pos >= meta.task_count {
        meta.current_queue_pos = meta.task_count - 1;
    }

    if let Some(cr3) = cleanup_cr3 {
        vmm::destroy_user_address_space(cr3);
    }

    true
}

/// Resets and initializes the round-robin scheduler.
///
/// This also registers the PIT IRQ handler that drives preemption.
pub fn init() {
    with_sched(|sched| {
        // `SCHED` is already constructed once at boot via `Scheduler::new()`.
        // We still reset only `meta` here so repeated `init()` calls start from
        // a clean scheduler state (idempotent re-init) without touching task stacks.
        sched.meta = SchedulerMetadata::new();
        sched.meta.initialized = true;
    });

    interrupts::register_irq_handler(interrupts::IRQ0_PIT_TIMER_VECTOR, timer_irq_handler);
}

/// Starts scheduling if initialized and at least one task is available.
pub fn start() {
    with_sched(|sched| {
        if sched.meta.initialized && sched.meta.task_count > 0 {
            sched.meta.started = true;
            sched.meta.stop_requested = false;
            sched.meta.bootstrap_frame = ptr::null_mut();
            sched.meta.running_slot = None;
            sched.meta.current_queue_pos = sched.meta.task_count - 1;
        }
    });
}

/// Creates a new kernel task and appends it to the run queue.
///
/// Thin wrapper around the shared spawn path for kernel-mode tasks.
pub fn spawn_kernel_task(entry: KernelTaskFn) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::Kernel { entry })
}

/// Creates a new user task with explicit user entry point and user stack pointer.
///
/// `entry_rip` and `user_rsp` are user-space virtual addresses in the task's
/// address space identified by `cr3`.
#[allow(dead_code)]
pub fn spawn_user_task(
    entry_rip: u64,
    user_rsp: u64,
    cr3: u64,
) -> Result<usize, SpawnError> {
    spawn_internal(SpawnKind::User {
        entry_rip,
        user_rsp,
        cr3,
    })
}

/// Shared task creation path used by both public spawn wrappers.
fn spawn_internal(kind: SpawnKind) -> Result<usize, SpawnError> {
    with_sched(|sched| {
        if !sched.meta.initialized {
            return Err(SpawnError::NotInitialized);
        }

        if sched.meta.task_count >= MAX_TASKS {
            return Err(SpawnError::CapacityExceeded);
        }

        let slot_idx = (0..MAX_TASKS)
            .find(|idx| !sched.meta.slots[*idx].used)
            .ok_or(SpawnError::CapacityExceeded)?;

        let (frame_ptr, cr3, user_rsp, kernel_rsp_top, is_user) = match kind {
            SpawnKind::Kernel { entry } => {
                let (frame_ptr, kernel_rsp_top) =
                    build_initial_kernel_task_frame(&mut sched.stacks, slot_idx, entry);
                (frame_ptr, 0, 0, kernel_rsp_top, false)
            }
            SpawnKind::User {
                entry_rip,
                user_rsp,
                cr3,
            } => {
                let (frame_ptr, kernel_rsp_top) =
                    build_initial_user_task_frame(&mut sched.stacks, slot_idx, entry_rip, user_rsp);
                (frame_ptr, cr3, user_rsp, kernel_rsp_top, true)
            }
        };

        sched.meta.slots[slot_idx] = TaskEntry {
            used: true,
            state: TaskState::Ready,
            frame_ptr,
            cr3,
            user_rsp,
            kernel_rsp_top,
            is_user,
        };
        sched.meta.run_queue[sched.meta.task_count] = slot_idx;
        sched.meta.task_count += 1;

        Ok(slot_idx)
    })
}

/// Requests a cooperative scheduler stop on the next timer tick.
#[allow(dead_code)]
pub fn request_stop() {
    with_sched(|sched| {
        if sched.meta.started {
            sched.meta.stop_requested = true;
        }
    });
}

/// Returns whether the scheduler is currently active.
#[allow(dead_code)]
pub fn is_running() -> bool {
    with_sched(|sched| sched.meta.started)
}

/// Enables per-task address-space switching.
///
/// `kernel_cr3` must be the physical PML4 address for kernel-mode execution.
/// Once enabled, selecting a user task switches to that task's `cr3`; selecting
/// a kernel task switches back to `kernel_cr3`.
pub fn set_kernel_address_space_cr3(kernel_cr3: u64) {
    with_sched(|sched| {
        sched.meta.address_space_switching_enabled = true;
        sched.meta.kernel_cr3 = kernel_cr3;
        sched.meta.active_cr3 = kernel_cr3;
    });
}

/// Disables per-task address-space switching.
#[allow(dead_code)]
pub fn disable_address_space_switching() {
    with_sched(|sched| {
        sched.meta.address_space_switching_enabled = false;
        sched.meta.kernel_cr3 = 0;
        sched.meta.active_cr3 = 0;
    });
}

/// IRQ adapter that routes PIT ticks into the scheduler core.
fn timer_irq_handler(_vector: u8, frame: &mut SavedRegisters) -> *mut SavedRegisters {
    on_timer_tick(frame as *mut SavedRegisters)
}

/// Switches CR3 to the selected task context when switching is enabled.
fn apply_selected_address_space(meta: &mut SchedulerMetadata, selected_slot: usize) {
    if !meta.address_space_switching_enabled {
        return;
    }

    let target_cr3 = if meta.slots[selected_slot].is_user {
        meta.slots[selected_slot].cr3
    } else {
        meta.kernel_cr3
    };

    if target_cr3 == 0 || meta.active_cr3 == target_cr3 {
        return;
    }

    // SAFETY:
    // - `target_cr3` originates from scheduler-controlled task metadata.
    // - Caller enables switching only after VMM initialization.
    unsafe {
        vmm::switch_page_directory(target_cr3);
    }
    meta.active_cr3 = target_cr3;
}

/// Removes all [`Zombie`](TaskState::Zombie) tasks from the run queue.
///
/// Called at the start of [`on_timer_tick`] — at that point execution has
/// already moved off the zombie's kernel stack (either onto a different
/// task's stack or onto the bootstrap stack), so freeing the slot is safe.
fn reap_zombies(meta: &mut SchedulerMetadata) {
    let mut i = 0;
    while i < meta.task_count {
        let slot = meta.run_queue[i];

        if meta.slots[slot].state == TaskState::Zombie {
            remove_task(meta, slot);
            // `remove_task` shifts entries down; re-check the same index.
            continue;
        }

        i += 1;
    }
}

/// Scheduler core executed on every timer IRQ.
///
/// The function saves current context (when known), selects the next runnable
/// task in round-robin order, and returns the frame pointer to resume.
pub fn on_timer_tick(current_frame: *mut SavedRegisters) -> *mut SavedRegisters {
    with_sched(|sched| {
        let meta = &mut sched.meta;

        if !meta.started {
            return current_frame;
        }

        // Reap zombie tasks first.  At this point execution is on a
        // different stack (bootstrap or another task), so freeing the
        // zombie's slot and stack is safe.
        reap_zombies(meta);

        if meta.task_count == 0 {
            meta.running_slot = None;
            if !meta.bootstrap_frame.is_null() {
                return meta.bootstrap_frame;
            }
            return current_frame;
        }

        let detected_slot = find_entry_by_frame(meta, &sched.stacks, current_frame);
        if detected_slot.is_none() && !frame_within_any_task_stack(&sched.stacks, current_frame) {
            // Always update bootstrap_frame to the latest non-task frame.
            // This is necessary because the boot stack layout may shift
            // between the initial capture (inside KernelMain) and later
            // ticks (inside idle_loop after the call), which would leave
            // bootstrap_frame pointing at a stale IRET frame with
            // corrupted CS/SS values.
            meta.bootstrap_frame = current_frame;
        }

        if meta.stop_requested {
            let return_frame = if !meta.bootstrap_frame.is_null() {
                meta.bootstrap_frame
            } else {
                current_frame
            };
            meta.started = false;
            meta.stop_requested = false;
            meta.bootstrap_frame = ptr::null_mut();
            meta.running_slot = None;
            meta.current_queue_pos = 0;
            meta.task_count = 0;
            meta.tick_count = 0;
            meta.run_queue = [0; MAX_TASKS];
            meta.slots = [TaskEntry::empty(); MAX_TASKS];
            return return_frame;
        }

        if let Some(slot) = detected_slot {
            // Save only when the interrupted frame can be mapped to a known task stack.
            meta.slots[slot].frame_ptr = current_frame;
        } else if let Some(running_slot) = meta.running_slot {
            // Unexpected frame source (not part of any task stack): keep running task.
            // This avoids corrupting RR state when called with a foreign frame pointer.
            return meta.slots[running_slot].frame_ptr;
        }

        let base_pos = if let Some(slot) = detected_slot {
            (0..meta.task_count)
                .find(|pos| meta.run_queue[*pos] == slot)
                .unwrap_or(meta.current_queue_pos)
        } else {
            meta.current_queue_pos
        };

        let search_start_pos = (base_pos + 1) % meta.task_count;

        let mut selected_pos = None;
        let mut selected_slot = 0usize;
        let mut selected_frame = ptr::null_mut();

        for step in 0..meta.task_count {
            let pos = (search_start_pos + step) % meta.task_count;
            let slot = meta.run_queue[pos];

            // Skip non-runnable tasks (blocked or zombie).
            if meta.slots[slot].state == TaskState::Blocked
                || meta.slots[slot].state == TaskState::Zombie
            {
                continue;
            }

            let frame = meta.slots[slot].frame_ptr;

            if meta.slots[slot].is_frame_within_stack(&sched.stacks, slot, frame) {
                selected_pos = Some(pos);
                selected_slot = slot;
                selected_frame = frame;
                break;
            }
        }

        meta.tick_count = meta.tick_count.wrapping_add(1);

        if let Some(pos) = selected_pos {
            meta.current_queue_pos = pos;
            meta.running_slot = Some(selected_slot);

            if meta.slots[selected_slot].is_user {
                gdt::set_kernel_rsp0(meta.slots[selected_slot].kernel_rsp_top);
            }

            apply_selected_address_space(meta, selected_slot);

            selected_frame
        } else if !meta.bootstrap_frame.is_null() {
            // All tasks are blocked — return to the idle loop so the CPU
            // can execute `hlt` instead of busy-spinning a blocked task.
            meta.running_slot = None;
            meta.bootstrap_frame
        } else {
            meta.running_slot = None;
            current_frame
        }
    })
}

/// Returns the saved frame pointer for `task_id` if that slot is active.
///
/// Primarily intended for integration tests and diagnostics.
pub fn task_frame_ptr(task_id: usize) -> Option<*mut SavedRegisters> {
    with_sched(|sched| {
        if task_id >= MAX_TASKS || !sched.meta.slots[task_id].used {
            None
        } else {
            Some(sched.meta.slots[task_id].frame_ptr)
        }
    })
}

/// Returns a copy of the initial interrupt return frame for `task_id`.
///
/// Intended for tests that validate kernel/user frame construction semantics.
#[allow(dead_code)]
pub fn task_iret_frame(task_id: usize) -> Option<InterruptStackFrame> {
    with_sched(|sched| {
        if task_id >= MAX_TASKS || !sched.meta.slots[task_id].used {
            return None;
        }
        let frame_ptr = sched.meta.slots[task_id].frame_ptr as usize;
        let iret_ptr = frame_ptr + size_of::<SavedRegisters>();
        // SAFETY:
        // - `frame_ptr` belongs to the scheduler-owned stack for this task.
        // - `InterruptStackFrame` is written directly behind `SavedRegisters`.
        Some(unsafe { *(iret_ptr as *const InterruptStackFrame) })
    })
}

/// Returns the slot index of the currently running task, if any.
pub fn current_task_id() -> Option<usize> {
    with_sched(|sched| sched.meta.running_slot)
}

/// Marks the task in `task_id` as [`TaskState::Blocked`].
///
/// A blocked task is skipped by the round-robin selector until it is
/// unblocked via [`unblock_task`].
pub fn block_task(task_id: usize) {
    with_sched(|sched| {
        if task_id < MAX_TASKS
            && sched.meta.slots[task_id].used
            && sched.meta.slots[task_id].state != TaskState::Blocked
        {
            sched.meta.slots[task_id].state = TaskState::Blocked;
        }
    });
}

/// Marks a previously blocked task as [`TaskState::Ready`].
///
/// Safe to call from IRQ context (the scheduler spinlock handles
/// interrupt masking internally).
pub fn unblock_task(task_id: usize) {
    with_sched(|sched| {
        if task_id < MAX_TASKS
            && sched.meta.slots[task_id].used
            && sched.meta.slots[task_id].state == TaskState::Blocked
        {
            sched.meta.slots[task_id].state = TaskState::Ready;
        }
    });
}

/// Terminates `task_id`, removing it from the run queue and freeing its slot.
///
/// Returns `true` if the task existed and was removed.
pub fn terminate_task(task_id: usize) -> bool {
    with_sched(|sched| remove_task(&mut sched.meta, task_id))
}

/// Marks an existing task as user-mode task context.
///
/// The scheduler uses `kernel_rsp_top` to update `TSS.RSP0` before resuming
/// this task, so future ring3->ring0 transitions enter on the task-specific
/// kernel stack.
#[allow(dead_code)]
pub fn set_task_user_context(task_id: usize, cr3: u64, user_rsp: u64, kernel_rsp_top: u64) -> bool {
    with_sched(|sched| {
        if task_id >= MAX_TASKS || !sched.meta.slots[task_id].used {
            return false;
        }

        let slot = &mut sched.meta.slots[task_id];
        slot.cr3 = cr3;
        slot.user_rsp = user_rsp;
        slot.kernel_rsp_top = kernel_rsp_top;
        slot.is_user = true;
        true
    })
}

/// Returns whether `task_id` is configured as a user-mode task.
#[allow(dead_code)]
pub fn is_user_task(task_id: usize) -> bool {
    with_sched(|sched| {
        task_id < MAX_TASKS && sched.meta.slots[task_id].used && sched.meta.slots[task_id].is_user
    })
}

/// Returns task context tuple `(cr3, user_rsp, kernel_rsp_top)` for `task_id`.
#[allow(dead_code)]
pub fn task_context(task_id: usize) -> Option<(u64, u64, u64)> {
    with_sched(|sched| {
        if task_id >= MAX_TASKS || !sched.meta.slots[task_id].used {
            None
        } else {
            let slot = &sched.meta.slots[task_id];
            Some((slot.cr3, slot.user_rsp, slot.kernel_rsp_top))
        }
    })
}

/// Returns the lifecycle state of `task_id`, or `None` if the slot is unused.
#[allow(dead_code)]
pub fn task_state(task_id: usize) -> Option<TaskState> {
    with_sched(|sched| {
        if task_id >= MAX_TASKS || !sched.meta.slots[task_id].used {
            None
        } else {
            Some(sched.meta.slots[task_id].state)
        }
    })
}

/// Marks the currently running task as [`TaskState::Zombie`].
///
/// The slot remains allocated (`used = true`) so no `spawn_*` call can
/// reuse it.  The scheduler skips zombie tasks during round-robin selection
/// and reaps them at the start of the next [`on_timer_tick`], when
/// execution has moved to a different stack.
///
/// # Panics
///
/// Panics if called outside a scheduled task context.
pub fn mark_current_as_zombie() {
    with_sched(|sched| {
        let slot = sched
            .meta
            .running_slot
            .expect("mark_current_as_zombie called outside scheduled task");
        sched.meta.slots[slot].state = TaskState::Zombie;
    });
}

/// Terminates the currently running task and forces an immediate reschedule.
///
/// The task is first marked as [`Zombie`](TaskState::Zombie) so its slot
/// and stack remain reserved.  The subsequent `yield_now` triggers a
/// context switch; the scheduler will never select this task again and
/// reaps the zombie slot on the following tick.
///
/// This two-phase approach eliminates the race window that existed when
/// the slot was freed before `yield_now`: a timer IRQ in that gap could
/// allow `spawn_*` to reuse the slot and overwrite the stack while the
/// exiting code was still running on it.
///
/// This function never returns.
pub fn exit_current_task() -> ! {
    mark_current_as_zombie();
    yield_now();
    loop {
        core::hint::spin_loop();
    }
}

/// Triggers a software timer interrupt to force an immediate reschedule.
pub fn yield_now() {
    // SAFETY:
    // - Software interrupt to IRQ0 vector enters the same scheduler path as timer IRQ.
    // - Valid only in ring 0, which holds for kernel code.
    unsafe {
        asm!(
            "int {vector}",
            vector = const interrupts::IRQ0_PIT_TIMER_VECTOR,
            options(nomem)
        );
    }
}
