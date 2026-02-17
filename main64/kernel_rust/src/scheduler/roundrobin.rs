//! Minimal kernel-mode round-robin scheduler.
//!
//! Task stacks are heap-allocated at spawn time and freed when tasks are
//! reaped, keeping the static footprint minimal.

use core::alloc::Layout;
use core::arch::asm;
use core::mem::size_of;
use core::ptr;

extern crate alloc;
use alloc::alloc as heap_alloc;

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
const STACK_ALIGNMENT: usize = 16;
const PAGE_SIZE: usize = 4096;
const KERNEL_CODE_SELECTOR: u64 = gdt::KERNEL_CODE_SELECTOR as u64;
const KERNEL_DATA_SELECTOR: u64 = gdt::KERNEL_DATA_SELECTOR as u64;
const USER_CODE_SELECTOR: u64 = gdt::USER_CODE_SELECTOR as u64;
const USER_DATA_SELECTOR: u64 = gdt::USER_DATA_SELECTOR as u64;

/// RFLAGS bit 9: Interrupt Enable Flag.
/// When set, the CPU will respond to maskable hardware interrupts.
const RFLAGS_IF: u64 = 1 << 9;

/// RFLAGS bit 1: Reserved bit (always 1 in x86_64).
/// Architectural requirement: this bit must be set in all RFLAGS values.
const RFLAGS_RESERVED: u64 = 1 << 1;

/// Default RFLAGS value for new user tasks.
/// - IF=1: Enable timer preemption
/// - Reserved=1: Required by architecture
/// - IOPL=0: I/O privilege level 0 (no direct I/O port access)
const DEFAULT_RFLAGS: u64 = RFLAGS_IF | RFLAGS_RESERVED;

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

    /// Task pool is full.
    CapacityExceeded,

    /// Heap allocation for the task stack failed.
    StackAllocationFailed,
}

/// Lifecycle state of a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is eligible for scheduling.
    Ready,

    /// Task is the one currently executing on the CPU.
    #[allow(dead_code)]
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
struct TaskEntry {
    /// Slot allocation flag in the task pool.
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
    cr3: u64,

    /// User-mode stack pointer for ring-3 resume (future user-task entry).
    /// Kernel-only tasks currently keep this at zero.
    user_rsp: u64,

    /// Top of this task's kernel stack, used to program `TSS.RSP0`
    /// before resuming a user-mode task.
    kernel_rsp_top: u64,

    /// Marks whether this task should be treated as user-mode context.
    /// When set, scheduler updates `TSS.RSP0` from `kernel_rsp_top`.
    is_user: bool,

    /// Code-page teardown policy for user tasks.
    ///
    /// `true` means user-code leaf PFNs are returned to PMM when the task CR3
    /// is destroyed. `false` keeps code PFNs reserved (alias-safe).
    release_user_code_pfns: bool,

    /// Base address of this task's heap-allocated kernel stack.
    stack_base: *mut u8,

    /// Size of the heap-allocated kernel stack in bytes.
    stack_size: usize,
    // TODO: FPU/SSE/AVX State Management
    //
    // Currently, no FPU state is preserved across context switches.
    // If user tasks use floating-point operations, this will cause register
    // corruption and undefined behavior.
    //
    // Possible solutions:
    // 1. **Lazy FPU switching** (recommended for efficiency):
    //    - Set CR0.TS (Task Switched) bit on every task switch
    //    - Trap #NM (Device Not Available) on first FP instruction
    //    - Save previous task's FPU state, restore current task's state
    //    - Clear CR0.TS to allow FPU access
    //    - Requires: 512-byte XSAVE area per task (aligned to 64 bytes)
    //
    // 2. **Eager FPU save/restore**:
    //    - Save FPU state on every context switch using XSAVE
    //    - Restore FPU state before resuming task using XRSTOR
    //    - Simpler but higher overhead (512 bytes copied every switch)
    //    - Requires: 512-byte XSAVE area per task (aligned to 64 bytes)
    //
    // 3. **Disable FPU in user mode**:
    //    - Set CR0.EM (Emulation) bit to trap all FP instructions
    //    - Generate #UD (Invalid Opcode) on FP use
    //    - Prevents silent corruption but limits user-mode capabilities
    //
    // When implementing, add:
    // - `fpu_state: Option<Box<FpuState>>` (lazily allocated)
    // - Or: `fpu_state: [u8; 512]` (aligned, always allocated)
    // - Or: `fpu_state_ptr: *mut FpuState` (external allocation)
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
            release_user_code_pfns: false,
            stack_base: ptr::null_mut(),
            stack_size: 0,
        }
    }

    /// Checks whether `frame_ptr` lies within this task's stack memory.
    fn is_frame_within_stack(&self, frame_ptr: *const SavedRegisters) -> bool {
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

    /// Stacks from terminated tasks awaiting deallocation.
    ///
    /// When a task is terminated via [`terminate_task`] while the scheduler is
    /// running, the stack cannot be freed immediately because the next
    /// `on_timer_tick` may still receive a stale frame pointer from that stack.
    /// Keeping the range here allows [`frame_within_any_task_stack`] to
    /// recognize stale task frames and avoid overwriting `bootstrap_frame`.
    /// The stacks are freed on the next [`on_timer_tick`] call.
    pending_free_stacks: [(*mut u8, usize); MAX_TASKS],

    /// Number of valid entries in `pending_free_stacks`.
    pending_free_count: usize,
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
            pending_free_stacks: [(ptr::null_mut(), 0); MAX_TASKS],
            pending_free_count: 0,
        }
    }
}

unsafe impl Send for SchedulerMetadata {}

// SAFETY:
// - `SchedulerMetadata` is only accessed behind `SpinLock<SchedulerMetadata>`.
// - Raw pointers in slots point into heap-allocated stacks and are only
//   read/written while holding the lock.
static SCHED: SpinLock<SchedulerMetadata> = SpinLock::new(SchedulerMetadata::new());

/// Aligns `value` down to the given power-of-two `align`.
#[inline]
const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

/// Executes `f` while holding the scheduler spinlock.
#[inline]
fn with_sched<R>(f: impl FnOnce(&mut SchedulerMetadata) -> R) -> R {
    let mut sched = SCHED.lock();
    f(&mut sched)
}

/// Allocates a stack from the kernel heap and touches every page.
///
/// Returns a pointer to the base of the allocated block, or null on failure.
/// The returned memory is 16-byte aligned and zero-touched on every page
/// boundary to force demand paging at allocation time rather than in IRQ
/// context.
fn allocate_task_stack() -> *mut u8 {
    // SAFETY:
    // - Layout is non-zero and alignment is a power of two.
    let layout = unsafe { Layout::from_size_align_unchecked(TASK_STACK_SIZE, STACK_ALIGNMENT) };
    // SAFETY:
    // - Layout has non-zero size.
    let ptr = unsafe { heap_alloc::alloc(layout) };
    if ptr.is_null() {
        return ptr::null_mut();
    }

    // Touch every page to force demand paging now, not during IRQ context.
    // SAFETY:
    // - `ptr` points to a valid allocation of `TASK_STACK_SIZE` bytes.
    unsafe {
        for page_off in (0..TASK_STACK_SIZE).step_by(PAGE_SIZE) {
            ptr::write_volatile(ptr.add(page_off), 0);
        }
    }

    ptr
}

/// Frees a heap-allocated task stack.
///
/// # Safety
///
/// `ptr` must have been returned by `allocate_task_stack` and must not be
/// used after this call.
unsafe fn free_task_stack(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let layout = Layout::from_size_align_unchecked(TASK_STACK_SIZE, STACK_ALIGNMENT);
    heap_alloc::dealloc(ptr, layout);
}

extern "C" fn task_return_trap() -> ! {
    exit_current_task()
}

/// Builds the initial kernel-task context on a heap-allocated stack.
///
/// Returns a pointer to the saved [`SavedRegisters`] used as scheduler context,
/// and the stack top address (for `kernel_rsp_top`).
fn build_initial_kernel_task_frame(
    stack_base: *mut u8,
    stack_size: usize,
    entry: KernelTaskFn,
) -> (*mut SavedRegisters, u64) {
    // SAFETY:
    // - `stack_base` points to a valid heap allocation of `stack_size` bytes.
    // - The caller guarantees exclusive access to this stack memory.
    unsafe {
        let stack_top = stack_base as usize + stack_size;

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

/// Builds an initial user-mode task context on a heap-allocated stack.
///
/// The saved interrupt frame is configured so that the next scheduler-selected
/// `iretq` transitions to ring 3 at `entry_rip` with user stack `user_rsp`.
fn build_initial_user_task_frame(
    stack_base: *mut u8,
    stack_size: usize,
    entry_rip: u64,
    user_rsp: u64,
) -> (*mut SavedRegisters, u64) {
    // SAFETY:
    // - `stack_base` points to a valid heap allocation of `stack_size` bytes.
    // - The caller guarantees exclusive access to this stack memory.
    unsafe {
        let stack_top = stack_base as usize + stack_size;

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
    frame_ptr: *const SavedRegisters,
) -> Option<usize> {
    if frame_ptr.is_null() {
        return None;
    }

    for pos in 0..meta.task_count {
        let slot = meta.run_queue[pos];
        if meta.slots[slot].used && meta.slots[slot].is_frame_within_stack(frame_ptr) {
            return Some(slot);
        }
    }

    None
}

/// Returns `true` when `frame_ptr` lies within any scheduler-owned task stack,
/// including stacks from recently terminated tasks that are pending deallocation.
fn frame_within_any_task_stack(meta: &SchedulerMetadata, frame_ptr: *const SavedRegisters) -> bool {
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
    for i in 0..meta.pending_free_count {
        let (base, size) = meta.pending_free_stacks[i];
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
/// Actual deallocation happens on the next [`on_timer_tick`] or in [`init`].
///
/// Returns `true` when an active task was removed.
fn remove_task(meta: &mut SchedulerMetadata, task_id: usize) -> bool {
    // Step 1: reject invalid or already-free task IDs.
    if task_id >= MAX_TASKS || !meta.slots[task_id].used {
        return false;
    }

    // Step 2: locate the task inside the compact run queue.
    // If it is not present, scheduler metadata is inconsistent for this ID.
    let Some(removed_pos) = (0..meta.task_count).find(|pos| meta.run_queue[*pos] == task_id) else {
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
        if meta.active_cr3 == cr3 {
            if meta.kernel_cr3 != 0 && meta.kernel_cr3 != cr3 {
                // Before destroying a user address space, ensure we are not
                // still executing with that CR3 active on this CPU.
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
                // Slot/stack cleanup still proceeds so scheduler state stays consistent.
                cleanup = None;
            }
        }
    }

    // Move the stack to the pending-free list instead of freeing it now.
    // This keeps the stack range visible to `frame_within_any_task_stack`
    // until the next timer tick, preventing stale task frames from being
    // mistaken for bootstrap frames.
    if !meta.slots[task_id].stack_base.is_null() && meta.pending_free_count < MAX_TASKS {
        meta.pending_free_stacks[meta.pending_free_count] = (
            meta.slots[task_id].stack_base,
            meta.slots[task_id].stack_size,
        );
        meta.pending_free_count += 1;
    }

    // Step 4: compact the run queue by shifting all entries after `removed_pos`.
    for pos in removed_pos..meta.task_count - 1 {
        meta.run_queue[pos] = meta.run_queue[pos + 1];
    }

    meta.run_queue[meta.task_count - 1] = 0;
    meta.task_count -= 1;

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
    if meta.task_count == 0 {
        meta.current_queue_pos = 0;
    } else if removed_pos < meta.current_queue_pos {
        meta.current_queue_pos -= 1;
    } else if meta.current_queue_pos >= meta.task_count {
        meta.current_queue_pos = meta.task_count - 1;
    }

    // Final step: release user address-space resources if cleanup is safe.
    if let Some((cr3, release_user_code_pfns)) = cleanup {
        vmm::destroy_user_address_space_with_options(cr3, release_user_code_pfns);
    }

    true
}

/// Resets and initializes the round-robin scheduler.
///
/// Any existing task stacks are freed before resetting the scheduler state.
/// This also registers the PIT IRQ handler that drives preemption.
pub fn init() {
    // Collect stacks to free while holding the lock, then free after release.
    let mut stacks_to_free: [(*mut u8, usize); MAX_TASKS] = [(ptr::null_mut(), 0); MAX_TASKS];
    let mut free_count = 0;

    with_sched(|meta| {
        // Free any stacks from a previous scheduler session.
        for slot in meta.slots.iter() {
            if slot.used && !slot.stack_base.is_null() && free_count < MAX_TASKS {
                stacks_to_free[free_count] = (slot.stack_base, slot.stack_size);
                free_count += 1;
            }
        }

        // Also drain any pending-free stacks from terminated tasks.
        for i in 0..meta.pending_free_count {
            if free_count < MAX_TASKS {
                stacks_to_free[free_count] = meta.pending_free_stacks[i];
                free_count += 1;
            }
        }

        *meta = SchedulerMetadata::new();
        meta.initialized = true;
    });

    // Free old stacks outside the scheduler lock.
    for &(ptr, _size) in &stacks_to_free[..free_count] {
        // SAFETY: Pointers were returned by `allocate_task_stack`.
        unsafe {
            free_task_stack(ptr);
        }
    }

    interrupts::register_irq_handler(interrupts::IRQ0_PIT_TIMER_VECTOR, timer_irq_handler);
}

/// Starts scheduling if initialized and at least one task is available.
pub fn start() {
    with_sched(|meta| {
        if meta.initialized && meta.task_count > 0 {
            meta.started = true;
            meta.stop_requested = false;
            meta.bootstrap_frame = ptr::null_mut();
            meta.running_slot = None;
            meta.current_queue_pos = meta.task_count - 1;
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

/// Shared task creation path used by both public spawn wrappers.
///
/// The stack is heap-allocated *before* acquiring the scheduler lock to
/// avoid nested spinlock acquisition (scheduler lock + heap lock).
fn spawn_internal(kind: SpawnKind) -> Result<usize, SpawnError> {
    // Pre-check: reject early if scheduler is not initialized or full,
    // before performing the (expensive) heap allocation.
    let pre_check = with_sched(|meta| {
        if !meta.initialized {
            return Err(SpawnError::NotInitialized);
        }
        if meta.task_count >= MAX_TASKS {
            return Err(SpawnError::CapacityExceeded);
        }
        Ok(())
    });
    pre_check?;

    // Allocate the stack outside the scheduler lock to avoid nesting
    // the scheduler spinlock with the heap spinlock.
    let stack_ptr = allocate_task_stack();
    if stack_ptr.is_null() {
        return Err(SpawnError::StackAllocationFailed);
    }

    let result = with_sched(|meta| {
        // Re-check under lock — state may have changed between pre-check and now.
        if !meta.initialized {
            return Err(SpawnError::NotInitialized);
        }

        if meta.task_count >= MAX_TASKS {
            return Err(SpawnError::CapacityExceeded);
        }

        let slot_idx = (0..MAX_TASKS)
            .find(|idx| !meta.slots[*idx].used)
            .ok_or(SpawnError::CapacityExceeded)?;

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

        meta.slots[slot_idx] = TaskEntry {
            used: true,
            state: TaskState::Ready,
            frame_ptr,
            cr3,
            user_rsp,
            kernel_rsp_top,
            is_user,
            release_user_code_pfns,
            stack_base: stack_ptr,
            stack_size: TASK_STACK_SIZE,
        };
        meta.run_queue[meta.task_count] = slot_idx;
        meta.task_count += 1;

        Ok(slot_idx)
    });

    // If spawn failed after we already allocated the stack, free it.
    if result.is_err() {
        // SAFETY: `stack_ptr` was returned by `allocate_task_stack` and has
        // not been stored in any task slot (spawn failed).
        unsafe {
            free_task_stack(stack_ptr);
        }
    }

    result
}

/// Requests a cooperative scheduler stop on the next timer tick.
#[cfg_attr(not(test), allow(dead_code))]
pub fn request_stop() {
    with_sched(|meta| {
        if meta.started {
            meta.stop_requested = true;
        }
    });
}

/// Returns whether the scheduler is currently active.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_running() -> bool {
    with_sched(|meta| meta.started)
}

/// Enables per-task address-space switching.
///
/// `kernel_cr3` must be the physical PML4 address for kernel-mode execution.
/// Once enabled, selecting a user task switches to that task's `cr3`; selecting
/// a kernel task switches back to `kernel_cr3`.
pub fn set_kernel_address_space_cr3(kernel_cr3: u64) {
    with_sched(|meta| {
        meta.address_space_switching_enabled = true;
        meta.kernel_cr3 = kernel_cr3;
        meta.active_cr3 = kernel_cr3;
    });
}

/// Disables per-task address-space switching.
#[cfg_attr(not(test), allow(dead_code))]
pub fn disable_address_space_switching() {
    with_sched(|meta| {
        meta.address_space_switching_enabled = false;
        meta.kernel_cr3 = 0;
        meta.active_cr3 = 0;
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
///
/// Zombie task stacks are moved to the pending-free list and will be
/// deallocated after releasing the scheduler lock.
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
    // Stacks to free after releasing the scheduler lock.
    let mut stacks_to_free = [(ptr::null_mut(), 0usize); MAX_TASKS];
    let mut free_count = 0;

    let result = {
        let mut sched = SCHED.lock();
        let meta = &mut *sched;

        if !meta.started {
            return current_frame;
        }

        // Reap zombie tasks.  At this point execution is on a different
        // stack (bootstrap or another task), so removing the zombie's slot
        // is safe.  Their stacks go to pending_free_stacks and will be
        // drained below, after the bootstrap frame detection.
        reap_zombies(meta);

        if meta.task_count == 0 {
            meta.running_slot = None;
            let frame = if !meta.bootstrap_frame.is_null() {
                meta.bootstrap_frame
            } else {
                current_frame
            };

            // Drain all pending-free stacks before returning.
            for i in 0..meta.pending_free_count {
                stacks_to_free[free_count] = meta.pending_free_stacks[i];
                free_count += 1;
            }
            meta.pending_free_count = 0;
            meta.pending_free_stacks = [(ptr::null_mut(), 0); MAX_TASKS];

            drop(sched);
            free_pending_stacks(&stacks_to_free[..free_count]);
            return frame;
        }

        let detected_slot = find_entry_by_frame(meta, current_frame);
        if detected_slot.is_none() && !frame_within_any_task_stack(meta, current_frame) {
            // Always update bootstrap_frame to the latest non-task frame.
            // This is necessary because the boot stack layout may shift
            // between the initial capture (inside KernelMain) and later
            // ticks (inside idle_loop after the call), which would leave
            // bootstrap_frame pointing at a stale IRET frame with
            // corrupted CS/SS values.
            meta.bootstrap_frame = current_frame;
        }

        // Now that bootstrap frame detection is done, drain pending-free
        // stacks for deallocation after the lock is released.
        for i in 0..meta.pending_free_count {
            if free_count < MAX_TASKS {
                stacks_to_free[free_count] = meta.pending_free_stacks[i];
                free_count += 1;
            }
        }
        meta.pending_free_count = 0;
        meta.pending_free_stacks = [(ptr::null_mut(), 0); MAX_TASKS];

        if meta.stop_requested {
            // Collect all remaining task stacks for deferred freeing.
            for slot_idx in 0..MAX_TASKS {
                if meta.slots[slot_idx].used
                    && !meta.slots[slot_idx].stack_base.is_null()
                    && free_count < MAX_TASKS
                {
                    stacks_to_free[free_count] = (
                        meta.slots[slot_idx].stack_base,
                        meta.slots[slot_idx].stack_size,
                    );
                    free_count += 1;
                }
            }
            // Also collect any zombie stacks that were just reaped.
            for i in 0..meta.pending_free_count {
                if free_count < MAX_TASKS {
                    stacks_to_free[free_count] = meta.pending_free_stacks[i];
                    free_count += 1;
                }
            }

            let return_frame = if !meta.bootstrap_frame.is_null() {
                meta.bootstrap_frame
            } else {
                current_frame
            };
            // Reset scheduling state but preserve `initialized` and
            // address-space configuration so the scheduler can be restarted.
            meta.started = false;
            meta.stop_requested = false;
            meta.bootstrap_frame = ptr::null_mut();
            meta.running_slot = None;
            meta.current_queue_pos = 0;
            meta.task_count = 0;
            meta.tick_count = 0;
            meta.run_queue = [0; MAX_TASKS];
            meta.slots = [TaskEntry::empty(); MAX_TASKS];
            meta.pending_free_stacks = [(ptr::null_mut(), 0); MAX_TASKS];
            meta.pending_free_count = 0;

            drop(sched);
            free_pending_stacks(&stacks_to_free[..free_count]);
            return return_frame;
        }

        if let Some(slot) = detected_slot {
            // Save only when the interrupted frame can be mapped to a known task stack.
            meta.slots[slot].frame_ptr = current_frame;
        } else if let Some(running_slot) = meta.running_slot {
            // Unexpected frame source (not part of any task stack): keep running task.
            // This avoids corrupting RR state when called with a foreign frame pointer.
            let frame = meta.slots[running_slot].frame_ptr;

            drop(sched);
            free_pending_stacks(&stacks_to_free[..free_count]);
            return frame;
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

            if meta.slots[slot].is_frame_within_stack(frame) {
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
    };

    // Free stacks from previous tick after releasing the scheduler lock.
    free_pending_stacks(&stacks_to_free[..free_count]);

    result
}

/// Frees heap-allocated task stacks outside the scheduler lock.
fn free_pending_stacks(stacks: &[(*mut u8, usize)]) {
    for &(ptr, _size) in stacks {
        // SAFETY: Pointers were returned by `allocate_task_stack`.
        unsafe {
            free_task_stack(ptr);
        }
    }
}

/// Returns the saved frame pointer for `task_id` if that slot is active.
///
/// Primarily intended for integration tests and diagnostics.
pub fn task_frame_ptr(task_id: usize) -> Option<*mut SavedRegisters> {
    with_sched(|meta| {
        if task_id >= MAX_TASKS || !meta.slots[task_id].used {
            None
        } else {
            Some(meta.slots[task_id].frame_ptr)
        }
    })
}

/// Returns a copy of the initial interrupt return frame for `task_id`.
///
/// Intended for tests that validate kernel/user frame construction semantics.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_iret_frame(task_id: usize) -> Option<InterruptStackFrame> {
    with_sched(|meta| {
        if task_id >= MAX_TASKS || !meta.slots[task_id].used {
            return None;
        }
        let frame_ptr = meta.slots[task_id].frame_ptr as usize;
        let iret_ptr = frame_ptr + size_of::<SavedRegisters>();
        // SAFETY:
        // - `frame_ptr` belongs to the scheduler-owned stack for this task.
        // - `InterruptStackFrame` is written directly behind `SavedRegisters`.
        Some(unsafe { *(iret_ptr as *const InterruptStackFrame) })
    })
}

/// Returns the slot index of the currently running task, if any.
pub fn current_task_id() -> Option<usize> {
    with_sched(|meta| meta.running_slot)
}

/// Marks the task in `task_id` as [`TaskState::Blocked`].
///
/// A blocked task is skipped by the round-robin selector until it is
/// unblocked via [`unblock_task`].
pub fn block_task(task_id: usize) {
    with_sched(|meta| {
        if task_id < MAX_TASKS
            && meta.slots[task_id].used
            && meta.slots[task_id].state != TaskState::Blocked
        {
            meta.slots[task_id].state = TaskState::Blocked;
        }
    });
}

/// Marks a previously blocked task as [`TaskState::Ready`].
///
/// Safe to call from IRQ context (the scheduler spinlock handles
/// interrupt masking internally).
pub fn unblock_task(task_id: usize) {
    with_sched(|meta| {
        if task_id < MAX_TASKS
            && meta.slots[task_id].used
            && meta.slots[task_id].state == TaskState::Blocked
        {
            meta.slots[task_id].state = TaskState::Ready;
        }
    });
}

/// Terminates `task_id`, removing it from the run queue and freeing its slot.
///
/// The task's stack is deferred for freeing on the next timer tick so that
/// stale frame pointers can still be detected by the scheduler.
///
/// Returns `true` if the task existed and was removed.
pub fn terminate_task(task_id: usize) -> bool {
    with_sched(|meta| remove_task(meta, task_id))
}

/// Waits cooperatively until `task_id` is no longer present in the scheduler.
///
/// This is intended for foreground command flows (for example REPL `exec`)
/// that need to block the caller until a spawned task has terminated.
///
/// Behavior:
/// - if `task_id` is already absent, this returns immediately,
/// - otherwise this repeatedly yields so normal scheduler ticks can progress.
pub fn wait_for_task_exit(task_id: usize) {
    wait_for_task_exit_with(task_id, |id| task_frame_ptr(id).is_some(), yield_now);
}

/// Generic wait helper behind [`wait_for_task_exit`].
///
/// `is_task_alive` must report whether `task_id` is still present.
/// `yield_once` must provide one cooperative scheduling opportunity.
///
/// Primarily exposed to keep the wait-loop contract directly testable without
/// requiring real interrupt-driven context switches in tests.
pub fn wait_for_task_exit_with<FAlive, FYield>(
    task_id: usize,
    mut is_task_alive: FAlive,
    mut yield_once: FYield,
) where
    FAlive: FnMut(usize) -> bool,
    FYield: FnMut(),
{
    // Foreground wait policy:
    // - poll liveness,
    // - yield between polls so the target task can run and eventually exit.
    while is_task_alive(task_id) {
        yield_once();
    }
}

/// Marks an existing task as user-mode task context.
///
/// The scheduler uses `kernel_rsp_top` to update `TSS.RSP0` before resuming
/// this task, so future ring3->ring0 transitions enter on the task-specific
/// kernel stack.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_task_user_context(task_id: usize, cr3: u64, user_rsp: u64, kernel_rsp_top: u64) -> bool {
    with_sched(|meta| {
        if task_id >= MAX_TASKS || !meta.slots[task_id].used {
            return false;
        }

        let slot = &mut meta.slots[task_id];
        slot.cr3 = cr3;
        slot.user_rsp = user_rsp;
        slot.kernel_rsp_top = kernel_rsp_top;
        slot.is_user = true;
        true
    })
}

/// Returns whether `task_id` is configured as a user-mode task.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_user_task(task_id: usize) -> bool {
    with_sched(|meta| {
        task_id < MAX_TASKS && meta.slots[task_id].used && meta.slots[task_id].is_user
    })
}

/// Returns task context tuple `(cr3, user_rsp, kernel_rsp_top)` for `task_id`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_context(task_id: usize) -> Option<(u64, u64, u64)> {
    with_sched(|meta| {
        if task_id >= MAX_TASKS || !meta.slots[task_id].used {
            None
        } else {
            let slot = &meta.slots[task_id];
            Some((slot.cr3, slot.user_rsp, slot.kernel_rsp_top))
        }
    })
}

/// Returns the lifecycle state of `task_id`, or `None` if the slot is unused.
#[cfg_attr(not(test), allow(dead_code))]
pub fn task_state(task_id: usize) -> Option<TaskState> {
    with_sched(|meta| {
        if task_id >= MAX_TASKS || !meta.slots[task_id].used {
            None
        } else {
            Some(meta.slots[task_id].state)
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
    with_sched(|meta| {
        let slot = meta
            .running_slot
            .expect("mark_current_as_zombie called outside scheduled task");
        meta.slots[slot].state = TaskState::Zombie;
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
