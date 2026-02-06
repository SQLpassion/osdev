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
use crate::drivers::keyboard;

pub type KernelTaskFn = extern "C" fn() -> !;

const MAX_TASKS: usize = 8;
const TASK_STACK_SIZE: usize = 64 * 1024;
const PAGE_SIZE: usize = 4096;
const KERNEL_CODE_SELECTOR: u64 = 0x08;
const KERNEL_DATA_SELECTOR: u64 = 0x10;
const DEFAULT_RFLAGS: u64 = 0x202;
const DEMO_SPIN_DELAY: u32 = 200_000;
const VGA_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;
const TRACE_ROW: usize = 17;
const DEMO_ROWS: [usize; 3] = [18, 19, 20];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    NotInitialized,
    CapacityExceeded,
}

#[derive(Clone, Copy)]
struct TaskSlot {
    used: bool,
    frame_ptr: *mut TrapFrame,
}

impl TaskSlot {
    const fn empty() -> Self {
        Self {
            used: false,
            frame_ptr: ptr::null_mut(),
        }
    }
}

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

struct SchedulerGlobal {
    state: UnsafeCell<SchedulerState>,
    stacks: UnsafeCell<[[u8; TASK_STACK_SIZE]; MAX_TASKS]>,
}

impl SchedulerGlobal {
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

#[inline]
const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

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

fn queue_pos_for_slot(state: &SchedulerState, slot: usize) -> Option<usize> {
    (0..state.task_count).find(|pos| state.run_queue[*pos] == slot)
}

#[inline]
pub const fn wrap_next_col(col: usize) -> usize {
    (col + 1) % VGA_COLS
}

fn timer_irq_handler(_vector: u8, frame: &mut TrapFrame) -> *mut TrapFrame {
    on_timer_tick(frame as *mut TrapFrame)
}

pub fn init() {
    with_state(|state| {
        *state = SchedulerState::new();
        state.initialized = true;
    });

    interrupts::register_irq_handler(interrupts::IRQ0_VECTOR, timer_irq_handler);
}

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

pub fn request_stop() {
    with_state(|state| {
        if state.started {
            state.stop_requested = true;
        }
    });
}

pub fn is_running() -> bool {
    with_state(|state| state.started)
}

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
        let trace_col = (state.tick_count as usize) % VGA_COLS;
        let trace_char = if let Some(pos) = selected_pos {
            let slot = state.run_queue[pos];
            b'0' + (slot as u8)
        } else {
            b'X'
        };

        // SAFETY:
        // - VGA text buffer is MMIO at `VGA_BUFFER`.
        // - Writes stay within visible row/column bounds.
        // - Volatile access is required for MMIO semantics.
        unsafe {
            let cell = VGA_BUFFER + (TRACE_ROW * VGA_COLS + trace_col) * 2;
            ptr::write_volatile(cell as *mut u8, trace_char);
            ptr::write_volatile((cell + 1) as *mut u8, 0x1E);
        }

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

#[allow(dead_code)]
pub fn yield_now() {
    // SAFETY:
    // - Software interrupt to IRQ0 vector enters the same scheduler path as timer IRQ.
    // - Valid only in ring 0, which holds for kernel code.
    unsafe {
        asm!(
            "int {vector}",
            vector = const interrupts::IRQ0_VECTOR,
            options(nomem)
        );
    }
}

macro_rules! demo_task_fn {
    ($name:ident, $row:expr, $ch:expr, $attr:expr) => {
        extern "C" fn $name() -> ! {
            let mut col = 0usize;
            let mut previous_col = VGA_COLS - 1;
            loop {
                keyboard::poll();
                if let Some(ch) = keyboard::read_char() {
                    if ch == b'q' || ch == b'Q' {
                        request_stop();
                    }
                }

                // SAFETY:
                // - VGA text buffer is MMIO at `VGA_BUFFER`.
                // - Writes stay within one fixed visible row.
                // - Volatile access is required for MMIO semantics.
                unsafe {
                    let old_cell = VGA_BUFFER + ($row * VGA_COLS + previous_col) * 2;
                    ptr::write_volatile(old_cell as *mut u8, b' ');
                    ptr::write_volatile((old_cell + 1) as *mut u8, $attr);

                    let cell = VGA_BUFFER + ($row * VGA_COLS + col) * 2;
                    ptr::write_volatile(cell as *mut u8, $ch);
                    ptr::write_volatile((cell + 1) as *mut u8, $attr);
                }
                previous_col = col;
                col = wrap_next_col(col);

                let mut delay = 0u32;
                while delay < DEMO_SPIN_DELAY {
                    core::hint::spin_loop();
                    delay += 1;
                }
            }
        }
    };
}

demo_task_fn!(demo_task_a, 18, b'A', 0x1F);
demo_task_fn!(demo_task_b, 19, b'B', 0x2F);
demo_task_fn!(demo_task_c, 20, b'C', 0x4F);

pub fn start_round_robin_demo() {
    // Invariant: no IRQ0 during rrdemo setup.
    //
    // Rationale:
    // - PIT programming is a multi-write I/O sequence (mode + low/high divisor bytes).
    // - If a timer IRQ preempts in the middle, we can leave setup early by switching
    //   away from the bootstrap context before the sequence/state is complete.
    // - On real hardware this can leave the PIT/scheduler startup in a broken state
    //   (often only one task keeps running). QEMU tends to be more forgiving.
    //
    // Therefore: keep IF=0 from here until all scheduler state is fully initialized.
    interrupts::disable();

    // SAFETY:
    // - VGA text buffer is MMIO at `VGA_BUFFER`.
    // - Writes are bounded to rows 18..20 and visible columns.
    // - Volatile writes preserve MMIO semantics.
    unsafe {
        for row in DEMO_ROWS {
            for col in 0..VGA_COLS {
                let cell = VGA_BUFFER + (row * VGA_COLS + col) * 2;
                ptr::write_volatile(cell as *mut u8, b' ');
                ptr::write_volatile((cell + 1) as *mut u8, 0x07);
            }
        }
    }

    init();
    let _ = spawn(demo_task_a).expect("rrdemo: spawn A failed");
    let _ = spawn(demo_task_b).expect("rrdemo: spawn B failed");
    let _ = spawn(demo_task_c).expect("rrdemo: spawn C failed");
    start();

    // Do not reprogram PIT here:
    // - KernelMain already configured 250 Hz.
    // - Reprogramming in this path would reintroduce the IRQ-vs-setup race above.
    //
    // Re-enable interrupts only after init/spawn/start are complete so the first
    // timer tick sees a consistent scheduler state.
    interrupts::enable();

    while is_running() {
        // SAFETY:
        // - While demo is running, interrupts are enabled and IRQ0 drives scheduling.
        // - `hlt` sleeps until the next interrupt and is valid in ring 0.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
