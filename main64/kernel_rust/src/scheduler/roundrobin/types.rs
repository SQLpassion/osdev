//! Type definitions and metadata structures for the round-robin scheduler.

use core::mem::size_of;
use core::ptr;

extern crate alloc;
use alloc::vec::Vec;

use crate::arch::fpu;
use crate::arch::interrupts::{InterruptStackFrame, SavedRegisters};

/// Entry point type for schedulable kernel tasks.
///
/// Tasks are entered via a synthetic interrupt-return frame and are expected
/// to never return.
pub type KernelTaskFn = extern "C" fn() -> !;

/// Internal task-construction descriptor for the shared spawn path.
///
/// Public APIs `spawn_kernel_task` and `spawn_user_task` are thin wrappers
/// that translate their parameters into one of these variants and call
/// `spawn_internal`.
pub enum SpawnKind {
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
        /// Whether user-code PFNs should be released on task teardown.
        ///
        /// `false` is used for temporary user aliases of kernel code pages.
        /// `true` is used for loader-owned binaries with dedicated code PFNs.
        release_user_code_pfns: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// Scheduler has not been initialized via [`init`].
    NotInitialized,

    /// Heap allocation for the task stack or scheduler metadata failed.
    StackAllocationFailed,
}

/// Lifecycle state of a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// One slot in the task table.
#[derive(Clone, Copy)]
pub struct TaskEntry {
    /// Slot allocation flag in the task pool.
    /// `true` means the entry is currently owned by a live task.
    pub used: bool,

    /// Scheduler lifecycle state used by round-robin selection.
    /// Blocked tasks are skipped until explicitly unblocked.
    pub state: TaskState,

    /// Pointer to the currently saved register frame for this task.
    /// This is the resume target returned to the IRQ trampoline.
    pub frame_ptr: *mut SavedRegisters,

    /// Task address space root (future user-mode CR3 switch support).
    /// Kernel-only tasks currently keep this at zero.
    pub cr3: u64,

    /// User-mode stack pointer snapshot kept for diagnostics/tests only.
    ///
    /// The scheduler resumes tasks from the saved `InterruptStackFrame` on the
    /// task stack; this field is not used in scheduling decisions.
    /// Kernel-only tasks keep this at zero.
    #[cfg_attr(not(test), allow(dead_code))]
    pub user_rsp: u64,

    /// Current top of the user-mode heap.
    /// Updated dynamically via `mmap` syscalls.
    /// Kernel-only tasks keep this at zero.
    pub user_heap_top: u64,

    /// Top of this task's kernel stack, used to program `TSS.RSP0`
    /// before resuming a user-mode task.
    pub kernel_rsp_top: u64,

    /// Marks whether this task should be treated as user-mode context.
    /// When set, scheduler updates `TSS.RSP0` from `kernel_rsp_top`.
    pub is_user: bool,

    /// Code-page teardown policy for user tasks.
    ///
    /// `true` means user-code leaf PFNs are returned to PMM when the task CR3
    /// is destroyed. `false` keeps code PFNs reserved (alias-safe).
    pub release_user_code_pfns: bool,

    /// Base address of this task's heap-allocated kernel stack.
    pub stack_base: *mut u8,

    /// Size of the heap-allocated kernel stack in bytes.
    pub stack_size: usize,

    /// Per-task FXSAVE64/FXRSTOR64 buffer for lazy FPU state switching.
    ///
    /// Allocated at spawn time (not lazily) to avoid heap allocation from the
    /// `#NM` exception context.  Freed by `remove_task`.  The buffer is
    /// initialised with a clean FPU state (equivalent to `FNINIT`) so tasks
    /// that never use FPU/SSE start from a well-defined state rather than
    /// inheriting random register contents.
    ///
    /// Raw pointer instead of `Box<FpuState>` because `TaskEntry: Copy`.
    pub fpu_state: *mut fpu::FpuState,
}

impl TaskEntry {
    /// Returns an unused slot marker.
    pub const fn empty() -> Self {
        Self {
            used: false,
            state: TaskState::Ready,
            frame_ptr: ptr::null_mut(),
            cr3: 0,
            user_rsp: 0,
            user_heap_top: 0,
            kernel_rsp_top: 0,
            is_user: false,
            release_user_code_pfns: false,
            stack_base: ptr::null_mut(),
            stack_size: 0,
            fpu_state: ptr::null_mut(),
        }
    }

    /// Checks whether `frame_ptr` lies within this task's stack memory.
    pub fn is_frame_within_stack(&self, frame_ptr: *const SavedRegisters) -> bool {
        if frame_ptr.is_null() || self.stack_base.is_null() {
            return false;
        }
        let frame_start = frame_ptr as usize;
        let frame_end =
            frame_start + size_of::<SavedRegisters>() + size_of::<InterruptStackFrame>();

        let stack_start = self.stack_base as usize;
        let stack_end = stack_start + self.stack_size;
        frame_start >= stack_start && frame_end <= stack_end
    }
}

/// Runtime metadata of the round-robin scheduler.
pub struct SchedulerMetadata {
    /// Global initialization latch set by [`init`].
    /// Guards API usage before scheduler data structures are ready.
    pub initialized: bool,

    /// Indicates whether timer ticks should perform scheduling decisions.
    /// Set by [`start`], cleared when scheduler state is reset.
    pub started: bool,

    /// Last non-task interrupt frame pointer (typically bootstrap/idle context).
    /// Used as fallback return frame when no runnable tasks exist.
    pub bootstrap_frame: *mut SavedRegisters,

    /// Slot index of currently selected/running task, if any.
    /// `None` when executing bootstrap/idle context.
    pub running_slot: Option<usize>,

    /// Cursor into `run_queue` used for round-robin progression.
    /// Points at the most recently selected queue position.
    pub current_queue_pos: usize,

    /// Compact queue of active task slot IDs in scheduling order.
    /// Length gives the number of active tasks.
    pub run_queue: Vec<usize>,

    /// Per-slot task metadata table.
    ///
    /// `used=false` marks free slots available for reuse by `spawn_internal`.
    /// The Vec grows when all existing slots are occupied. After a task exits,
    /// any trailing `used=false` entries are trimmed by `remove_task` so that
    /// the Vec length reflects the number of live tasks after every removal.
    ///
    /// Trade-off (explicit):
    /// - This is intentionally a `Vec + used-flag` design, not a dedicated
    ///   slot allocator/free-list.
    /// - Interior unused holes are reused by first-fit spawn, but they are not
    ///   compacted out of `slots`.
    /// - Under churny spawn/despawn patterns, `slots.len()` follows a high-water
    ///   mark and only shrinks when trailing slots become unused.
    pub slots: Vec<TaskEntry>,

    /// Total number of timer ticks processed while scheduler is started.
    /// Primarily for diagnostics/tests.
    pub tick_count: u64,

    /// Stacks from terminated tasks awaiting deallocation.
    ///
    /// When a task is terminated via [`terminate_task`] while the scheduler is
    /// running, the stack cannot be freed immediately because the next
    /// `on_timer_tick` may still receive a stale frame pointer from that stack.
    /// Keeping the range here allows [`frame_within_any_task_stack`] to
    /// recognize stale task frames and avoid overwriting `bootstrap_frame`.
    /// The stacks are freed on the next [`on_timer_tick`] call.
    /// Drained via `core::mem::take` inside the scheduler lock.
    pub pending_free_stacks: Vec<(*mut u8, usize)>,

    /// Slot index of the task whose FPU/SSE state is currently live in the
    /// CPU's XMM/x87 registers.
    ///
    /// `None` means no task owns the FPU (initial state after boot or after
    /// a context switch before any FPU instruction has been executed).
    ///
    /// Set to `Some(slot)` by [`handle_fpu_trap`] when the `#NM` handler
    /// restores a task's state.  Cleared to `None` by `select_next_task`
    /// after saving the outgoing owner's state via `FXSAVE64`.
    pub fpu_owner: Option<usize>,
}

impl SchedulerMetadata {
    /// Returns the initial scheduler metadata.
    ///
    /// `Vec::new()` does not allocate, so this is safe as a `const fn` and
    /// can be used to initialize the global `static SCHED`.
    pub const fn new() -> Self {
        Self {
            initialized: false,
            started: false,
            bootstrap_frame: ptr::null_mut(),
            running_slot: None,
            current_queue_pos: 0,
            run_queue: Vec::new(),
            slots: Vec::new(),
            tick_count: 0,
            pending_free_stacks: Vec::new(),
            fpu_owner: None,
        }
    }
}

impl Default for SchedulerMetadata {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - `SchedulerMetadata` is only accessed behind `SpinLock<SchedulerMetadata>`.
// - Raw pointers inside metadata always reference scheduler-owned task stacks.
// - Cross-thread transfer does not create unsynchronized mutable aliasing.
unsafe impl Send for SchedulerMetadata {}

/// Architecture-specific callbacks used by scheduler core.
///
/// This isolates MMU/TSS details from round-robin selection logic and makes
/// behavior replaceable in tests without modifying scheduler internals.
#[derive(Clone, Copy)]
pub struct SchedulerArchCallbacks {
    /// Returns the canonical kernel address-space root (CR3).
    pub read_kernel_cr3: fn() -> u64,
    /// Programs TSS.RSP0 before resuming a user-mode task.
    pub set_kernel_rsp0: fn(u64),
    /// Switches CPU address space to `cr3`.
    pub switch_cr3: unsafe fn(u64),
}
