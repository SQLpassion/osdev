//! Minimal kernel-mode round-robin scheduler.
//!
//! Phase 2 scope:
//! - static task pool (no heap allocations)
//! - timer-driven round-robin on IRQ0
//! - kernel-mode function pointers as task entries

use core::arch::asm;
use core::mem::size_of;
use core::ptr;

use crate::arch::interrupts::{self, InterruptStackFrame, SavedRegisters};
use crate::sync::spinlock::SpinLock;

/// Entry point type for schedulable kernel tasks.
///
/// Tasks are entered via a synthetic interrupt-return frame and are expected
/// to never return.
pub type KernelTaskFn = extern "C" fn() -> !;

const MAX_TASKS: usize = 8;
const TASK_STACK_SIZE: usize = 64 * 1024;
const PAGE_SIZE: usize = 4096;
const KERNEL_CODE_SELECTOR: u64 = 0x08;
const KERNEL_DATA_SELECTOR: u64 = 0x10;
const DEFAULT_RFLAGS: u64 = 0x202;

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
}

/// One slot in the static task table.
#[derive(Clone, Copy)]
struct TaskEntry {
    used: bool,
    state: TaskState,
    frame_ptr: *mut SavedRegisters,
}

impl TaskEntry {
    /// Returns an unused slot marker.
    const fn empty() -> Self {
        Self {
            used: false,
            state: TaskState::Ready,
            frame_ptr: ptr::null_mut(),
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
        let frame_end = frame_start + size_of::<SavedRegisters>() + size_of::<InterruptStackFrame>();

        let stack = &stacks[slot_idx];
        let stack_start = stack.as_ptr() as usize;
        let stack_end = stack_start + TASK_STACK_SIZE;
        frame_start >= stack_start && frame_end <= stack_end
    }
}

/// Runtime metadata of the round-robin scheduler.
struct SchedulerMetadata {
    initialized: bool,
    started: bool,
    stop_requested: bool,
    bootstrap_frame: *mut SavedRegisters,
    running_slot: Option<usize>,
    current_queue_pos: usize,
    task_count: usize,
    run_queue: [usize; MAX_TASKS],
    slots: [TaskEntry; MAX_TASKS],
    tick_count: u64,
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

/// Builds the initial task context on the stack of `slot_idx`.
///
/// Returns a pointer to the saved [`SavedRegisters`] used as scheduler context.
fn build_initial_task_frame(
    stacks: &mut [[u8; TASK_STACK_SIZE]; MAX_TASKS],
    slot_idx: usize,
    entry: KernelTaskFn,
) -> *mut SavedRegisters {
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
        ptr::write(entry_rsp as *mut u64, task_return_trap as *const () as usize as u64);

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

        frame_ptr
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
/// Returns the allocated task slot index on success.
pub fn spawn(entry: KernelTaskFn) -> Result<usize, SpawnError> {
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

        let frame_ptr = build_initial_task_frame(&mut sched.stacks, slot_idx, entry);
        sched.meta.slots[slot_idx] = TaskEntry {
            used: true,
            state: TaskState::Ready,
            frame_ptr,
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

/// IRQ adapter that routes PIT ticks into the scheduler core.
fn timer_irq_handler(_vector: u8, frame: &mut SavedRegisters) -> *mut SavedRegisters {
    on_timer_tick(frame as *mut SavedRegisters)
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

        if meta.task_count == 0 {
            meta.running_slot = None;
            if !meta.bootstrap_frame.is_null() {
                return meta.bootstrap_frame;
            }
            return current_frame;
        }

        let detected_slot = find_entry_by_frame(meta, &sched.stacks, current_frame);
        if detected_slot.is_none() {
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

            // Skip blocked tasks — they are waiting for an external event.
            if meta.slots[slot].state == TaskState::Blocked {
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
#[allow(dead_code)]
pub fn task_frame_ptr(task_id: usize) -> Option<*mut SavedRegisters> {
    with_sched(|sched| {
        if task_id >= MAX_TASKS || !sched.meta.slots[task_id].used {
            None
        } else {
            Some(sched.meta.slots[task_id].frame_ptr)
        }
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

/// Terminates the currently running task and forces an immediate reschedule.
///
/// This function never returns.
pub fn exit_current_task() -> ! {
    let task_id = current_task_id().expect("exit_current_task called outside scheduled task");
    let _ = terminate_task(task_id);
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
