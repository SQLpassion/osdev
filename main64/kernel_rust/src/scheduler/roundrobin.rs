//! Minimal kernel-mode round-robin scheduler.
//!
//! Phase 2 scope:
//! - static task pool (no heap allocations)
//! - timer-driven round-robin on IRQ0
//! - kernel-mode function pointers as task entries

use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::size_of;
use core::ptr;

use crate::arch::interrupts::{self, InterruptStackFrame, TrapFrame};

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

/// One slot in the static task table.
#[derive(Clone, Copy)]
struct TaskSlot {
    used: bool,
    frame_ptr: *mut TrapFrame,
}

impl TaskSlot {
    /// Returns an unused slot marker.
    const fn empty() -> Self {
        Self {
            used: false,
            frame_ptr: ptr::null_mut(),
        }
    }
}

/// Runtime state of the round-robin scheduler.
struct SchedulerState {
    initialized: bool,
    started: bool,
    stop_requested: bool,
    bootstrap_frame: *mut TrapFrame,
    running_slot: Option<usize>,
    current_queue_pos: usize,
    task_count: usize,
    run_queue: [usize; MAX_TASKS],
    slots: [TaskSlot; MAX_TASKS],
    tick_count: u64,
}

impl SchedulerState {
    /// Returns the initial scheduler state.
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
            slots: [TaskSlot::empty(); MAX_TASKS],
            tick_count: 0,
        }
    }
}

/// Global scheduler storage containing mutable state and per-task stacks.
struct SchedulerGlobal {
    state: UnsafeCell<SchedulerState>,
    stacks: UnsafeCell<[[u8; TASK_STACK_SIZE]; MAX_TASKS]>,
}

impl SchedulerGlobal {
    /// Creates zero-initialized global storage.
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(SchedulerState::new()),
            stacks: UnsafeCell::new([[0; TASK_STACK_SIZE]; MAX_TASKS]),
        }
    }
}

// SAFETY:
// - Kernel is currently single-core.
// - Access is serialized via interrupt masking in `with_state`.
unsafe impl Sync for SchedulerGlobal {}

static SCHED: SchedulerGlobal = SchedulerGlobal::new();

/// Executes `f` with mutable scheduler state while interrupts are masked.
///
/// Interrupt enablement is restored to its previous state afterwards.
#[inline]
fn with_state<R>(f: impl FnOnce(&mut SchedulerState) -> R) -> R {
    let interrupts_were_enabled = interrupts::are_enabled();
    interrupts::disable();

    // SAFETY:
    // - Access is protected by interrupt masking on a single core.
    // - No concurrent mutable access can occur while interrupts stay disabled.
    let result = unsafe { f(&mut *SCHED.state.get()) };

    if interrupts_were_enabled {
        interrupts::enable();
    }
    result
}

/// Aligns `value` down to the given power-of-two `align`.
#[inline]
const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

/// Builds the initial task context on the stack of `slot_idx`.
///
/// Returns a pointer to the saved [`TrapFrame`] used as scheduler context.
fn build_initial_task_frame(slot_idx: usize, entry: KernelTaskFn) -> *mut TrapFrame {
    // SAFETY:
    // - `slot_idx` is validated by caller to be in-bounds and unique.
    // - Each slot owns a disjoint stack region in `SCHED.stacks`.
    unsafe {
        let stacks = &mut *SCHED.stacks.get();
        let stack = &mut stacks[slot_idx];
        let stack_base = stack.as_mut_ptr() as usize;
        let stack_top = stack_base + TASK_STACK_SIZE;

        // Touch every stack page before first context switch.
        // This forces demand paging (if any) during spawn-time instead of in IRQ context.
        for page_off in (0..TASK_STACK_SIZE).step_by(PAGE_SIZE) {
            ptr::write_volatile(stack.as_mut_ptr().add(page_off), 0);
        }

        // SysV-friendly entry stack alignment.
        let entry_rsp = align_down(stack_top, 16) - 8;
        let iret_addr = entry_rsp - size_of::<InterruptStackFrame>();
        let frame_addr = iret_addr - size_of::<TrapFrame>();

        let frame_ptr = frame_addr as *mut TrapFrame;
        let iret_ptr = iret_addr as *mut InterruptStackFrame;

        ptr::write(frame_ptr, TrapFrame::default());
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

/// Checks whether `frame_ptr` points into the stack owned by `slot_idx`.
fn frame_in_slot_stack(slot_idx: usize, frame_ptr: *const TrapFrame) -> bool {
    if frame_ptr.is_null() {
        return false;
    }
    let frame_start = frame_ptr as usize;
    let frame_end = frame_start + size_of::<TrapFrame>() + size_of::<InterruptStackFrame>();

    // SAFETY:
    // - Read-only traversal of one scheduler-owned stack region.
    // - Single-core execution and interrupt masking at call sites prevent races.
    unsafe {
        let stacks = &*SCHED.stacks.get();
        let stack = &stacks[slot_idx];
        let stack_start = stack.as_ptr() as usize;
        let stack_end = stack_start + TASK_STACK_SIZE;
        frame_start >= stack_start && frame_end <= stack_end
    }
}

/// Resolves a trap frame pointer back to its owning task slot.
fn slot_for_frame(state: &SchedulerState, frame_ptr: *const TrapFrame) -> Option<usize> {
    if frame_ptr.is_null() {
        return None;
    }

    for pos in 0..state.task_count {
        let slot = state.run_queue[pos];
        if state.slots[slot].used && frame_in_slot_stack(slot, frame_ptr) {
            return Some(slot);
        }
    }

    None
}

/// Finds the run-queue position for a given task slot index.
fn queue_pos_for_slot(state: &SchedulerState, slot: usize) -> Option<usize> {
    (0..state.task_count).find(|pos| state.run_queue[*pos] == slot)
}

/// IRQ adapter that routes PIT ticks into the scheduler core.
fn timer_irq_handler(_vector: u8, frame: &mut TrapFrame) -> *mut TrapFrame {
    on_timer_tick(frame as *mut TrapFrame)
}

/// Resets and initializes the round-robin scheduler.
///
/// This also registers the PIT IRQ handler that drives preemption.
pub fn init() {
    with_state(|state| {
        *state = SchedulerState::new();
        state.initialized = true;
    });

    interrupts::register_irq_handler(interrupts::IRQ0_PIT_TIMER_VECTOR, timer_irq_handler);
}

/// Creates a new kernel task and appends it to the run queue.
///
/// Returns the allocated task slot index on success.
pub fn spawn(entry: KernelTaskFn) -> Result<usize, SpawnError> {
    with_state(|state| {
        if !state.initialized {
            return Err(SpawnError::NotInitialized);
        }

        if state.task_count >= MAX_TASKS {
            return Err(SpawnError::CapacityExceeded);
        }

        let slot_idx = (0..MAX_TASKS)
            .find(|idx| !state.slots[*idx].used)
            .ok_or(SpawnError::CapacityExceeded)?;

        let frame_ptr = build_initial_task_frame(slot_idx, entry);
        state.slots[slot_idx] = TaskSlot {
            used: true,
            frame_ptr,
        };
        state.run_queue[state.task_count] = slot_idx;
        state.task_count += 1;

        Ok(slot_idx)
    })
}

/// Starts scheduling if initialized and at least one task is available.
pub fn start() {
    with_state(|state| {
        if state.initialized && state.task_count > 0 {
            state.started = true;
            state.stop_requested = false;
            state.bootstrap_frame = ptr::null_mut();
            state.running_slot = None;
            state.current_queue_pos = state.task_count - 1;
        }
    });
}

/// Requests a cooperative scheduler stop on the next timer tick.
pub fn request_stop() {
    with_state(|state| {
        if state.started {
            state.stop_requested = true;
        }
    });
}

/// Returns whether the scheduler is currently active.
pub fn is_running() -> bool {
    with_state(|state| state.started)
}

/// Scheduler core executed on every timer IRQ.
///
/// The function saves current context (when known), selects the next runnable
/// task in round-robin order, and returns the frame pointer to resume.
pub fn on_timer_tick(current_frame: *mut TrapFrame) -> *mut TrapFrame {
    with_state(|state| {
        if !state.started || state.task_count == 0 {
            return current_frame;
        }

        let detected_slot = slot_for_frame(state, current_frame);
        if state.bootstrap_frame.is_null() && detected_slot.is_none() {
            state.bootstrap_frame = current_frame;
        }

        if state.stop_requested {
            let return_frame = if !state.bootstrap_frame.is_null() {
                state.bootstrap_frame
            } else {
                current_frame
            };
            state.started = false;
            state.stop_requested = false;
            state.bootstrap_frame = ptr::null_mut();
            state.running_slot = None;
            state.current_queue_pos = 0;
            state.task_count = 0;
            state.tick_count = 0;
            state.run_queue = [0; MAX_TASKS];
            state.slots = [TaskSlot::empty(); MAX_TASKS];
            return return_frame;
        }

        if let Some(slot) = detected_slot {
            // Save only when the interrupted frame can be mapped to a known task stack.
            state.slots[slot].frame_ptr = current_frame;
        } else if let Some(running_slot) = state.running_slot {
            // Unexpected frame source (not part of any task stack): keep running task.
            // This avoids corrupting RR state when called with a foreign frame pointer.
            return state.slots[running_slot].frame_ptr;
        }

        let base_pos = if let Some(slot) = detected_slot {
            queue_pos_for_slot(state, slot).unwrap_or(state.current_queue_pos)
        } else {
            state.current_queue_pos
        };

        let search_start_pos = (base_pos + 1) % state.task_count;

        let mut selected_pos = None;
        let mut selected_slot = 0usize;
        let mut selected_frame = ptr::null_mut();

        for step in 0..state.task_count {
            let pos = (search_start_pos + step) % state.task_count;
            let slot = state.run_queue[pos];
            let frame = state.slots[slot].frame_ptr;
            
            if frame_in_slot_stack(slot, frame) {
                selected_pos = Some(pos);
                selected_slot = slot;
                selected_frame = frame;
                break;
            }
        }

        state.tick_count = state.tick_count.wrapping_add(1);

        if let Some(pos) = selected_pos {
            state.current_queue_pos = pos;
            state.running_slot = Some(selected_slot);
            selected_frame
        } else if let Some(slot) = state.running_slot {
            state.slots[slot].frame_ptr
        } else {
            state.running_slot = None;
            current_frame
        }
    })
}

/// Returns the saved frame pointer for `task_id` if that slot is active.
///
/// Primarily intended for integration tests and diagnostics.
#[allow(dead_code)]
pub fn task_frame_ptr(task_id: usize) -> Option<*mut TrapFrame> {
    with_state(|state| {
        if task_id >= MAX_TASKS || !state.slots[task_id].used {
            None
        } else {
            Some(state.slots[task_id].frame_ptr)
        }
    })
}

/// Triggers a software timer interrupt to force an immediate reschedule.
#[allow(dead_code)]
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
