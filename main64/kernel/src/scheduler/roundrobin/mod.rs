//! # Round-Robin Kernel Task Scheduler
//!
//! A minimal, preemption-capable, round-robin task scheduler for a 64-bit Rust kernel running in ring 0.
//!
//! ## Core Features & Mechanics
//!
//! - **Preemption & Yielding**: Preemptive scheduling is driven by the PIT timer interrupt (IRQ0). Each tick triggers
//!   a context switch to the next ready task. Cooperative yields can be requested via the `yield_now` syscall.
//! - **Task Lifecycle**: Tasks cycle through `Ready`, `Running`, `Blocked`, and `Zombie` states. Terminated
//!   tasks are kept as `Zombie` slots and reaped in a deferred, two-phase cleanup on the next timer tick
//!   to avoid use-after-free races on the active stack.
//! - **Lazy FPU Switching**: Rather than saving the 512-byte FPU/SSE state on every switch, the scheduler
//!   sets the `CR0.TS` bit. The first FPU instruction by a task triggers a Device Not Available (`#NM`) trap,
//!   which restores the FPU context on-demand.
//! - **Heap-Allocated Stacks**: Stacks are allocated from the kernel heap, 16-byte aligned, and zero-touched
//!   on spawn to force demand paging allocation up front.
//! - **Architecture Decoupling**: Decouples TSS and MMU details from the core round-robin logic using the
//!   `SchedulerArchCallbacks` interface.

use core::arch::asm;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

#[cfg(debug_assertions)]
use core::sync::atomic::{AtomicBool, Ordering};

extern crate alloc;
use alloc::vec::Vec;

use crate::arch::fpu;
use crate::arch::gdt;
use crate::arch::interrupts::{self, SavedRegisters};
use crate::memory::vmm;
use crate::sync::spinlock::SpinLock;

use crate::sync::waitqueue_adapter;

pub mod types;
pub use types::*;

mod api;
mod context;
mod manager;
mod spawn;
mod wait;

#[allow(unused_imports)]
pub use api::{
    current_task_id, current_user_heap_top, is_user_task, reset_initialization_for_test,
    set_current_user_heap_top, set_task_user_context, slot_table_len, task_context, task_frame_ptr,
    task_iret_frame, task_state,
};
#[allow(unused_imports)]
pub use spawn::{spawn_kernel_task, spawn_user_task, spawn_user_task_owning_code};
#[allow(unused_imports)]
pub use wait::{
    block_task, terminate_task, unblock_task, wait_for_task_exit, wait_for_task_exit_with,
};

#[cfg(debug_assertions)]
use manager::reset_scheduler_state;
use manager::{
    bootstrap_or_current, find_entry_by_frame, frame_within_any_task_stack, free_pending_stacks,
    reap_zombies, select_next_task, take_pending_stacks_for_free,
};

/// Returns whether the scheduler is currently active.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_running() -> bool {
    with_scheduler(|meta| meta.started)
}

pub(crate) const TASK_STACK_SIZE: usize = 64 * 1024;
pub(crate) const STACK_ALIGNMENT: usize = 16;
pub(crate) const KERNEL_CODE_SELECTOR: u64 = gdt::KERNEL_CODE_SELECTOR as u64;
pub(crate) const KERNEL_DATA_SELECTOR: u64 = gdt::KERNEL_DATA_SELECTOR as u64;
pub(crate) const USER_CODE_SELECTOR: u64 = gdt::USER_CODE_SELECTOR as u64;
pub(crate) const USER_DATA_SELECTOR: u64 = gdt::USER_DATA_SELECTOR as u64;

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
pub(crate) const DEFAULT_RFLAGS: u64 = RFLAGS_IF | RFLAGS_RESERVED;

static SCHED: SpinLock<SchedulerMetadata> = SpinLock::new(SchedulerMetadata::new());

fn default_read_kernel_cr3() -> u64 {
    vmm::get_pml4_address()
}

fn default_set_kernel_rsp0(rsp0: u64) {
    gdt::set_kernel_rsp0(rsp0);
}

unsafe fn default_switch_cr3(cr3: u64) {
    vmm::switch_page_directory(cr3);
}

impl SchedulerArchCallbacks {
    const fn default_callbacks() -> Self {
        Self {
            read_kernel_cr3: default_read_kernel_cr3,
            set_kernel_rsp0: default_set_kernel_rsp0,
            switch_cr3: default_switch_cr3,
        }
    }
}

/// Runtime-selected architecture backend.
static SCHED_ARCH_CALLBACKS: SpinLock<SchedulerArchCallbacks> =
    SpinLock::new(SchedulerArchCallbacks::default_callbacks());

/// Kernel CR3 currently configured for kernel task selections.
static SCHED_KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

/// Last CR3 switched by scheduler path (for redundant-switch elimination).
static SCHED_ACTIVE_CR3: AtomicU64 = AtomicU64::new(0);

/// Test-only cooperative stop hook.
///
/// The production scheduler does not carry stop/restart control state in
/// `SchedulerMetadata`.  Debug/test builds keep this external hook so
/// scheduler integration tests can still terminate cleanly.
#[cfg(debug_assertions)]
pub(crate) static TEST_STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Executes `f` while holding the scheduler spinlock.
pub(super) fn with_scheduler<R>(f: impl FnOnce(&mut SchedulerMetadata) -> R) -> R {
    let mut sched = SCHED.lock();
    f(&mut sched)
}

pub(crate) fn arch_callbacks() -> SchedulerArchCallbacks {
    *SCHED_ARCH_CALLBACKS.lock()
}

pub(crate) fn kernel_cr3_value() -> u64 {
    SCHED_KERNEL_CR3.load(AtomicOrdering::Acquire)
}

pub(crate) fn active_cr3_value() -> u64 {
    SCHED_ACTIVE_CR3.load(AtomicOrdering::Acquire)
}

pub(crate) fn set_kernel_and_active_cr3(cr3: u64) {
    SCHED_KERNEL_CR3.store(cr3, AtomicOrdering::Release);
    SCHED_ACTIVE_CR3.store(cr3, AtomicOrdering::Release);
}

pub(crate) fn set_active_cr3(cr3: u64) {
    SCHED_ACTIVE_CR3.store(cr3, AtomicOrdering::Release);
}

/// Resets and initializes the round-robin scheduler.
///
/// Any existing task stacks are freed before resetting the scheduler state.
/// This also registers the PIT IRQ handler that drives preemption.
pub fn init() {
    // Collect stacks to free while holding the lock, then free after release.
    // `mem::take` on `pending_free_stacks` gives us the Vec with zero allocation;
    // active slot stacks are pushed into it via try_reserve to avoid OOM panics
    // inside the spinlock (where interrupts are disabled).
    let mut stacks_to_free: Vec<(*mut u8, usize)> = Vec::new();

    with_scheduler(|meta| {
        // Start with any pending-free stacks (no allocation via mem::take).
        stacks_to_free = core::mem::take(&mut meta.pending_free_stacks);

        // Collect stacks from all active slots into stacks_to_free.
        // Free FPU state buffers immediately (safe to do under the spinlock).
        // Use try_reserve(1) to avoid a potential panic (via the alloc-error
        // handler) inside the spinlock, where interrupts are already disabled.
        // An OOM here leaks one 64 KiB stack per failed reservation, but that
        // is far safer than a panic with interrupts disabled.
        for slot in meta.slots.iter_mut() {
            if slot.used {
                // SAFETY:
                // - This requires `unsafe` because it frees a raw heap allocation.
                // - `fpu_state` was returned by `fpu::FpuState::allocate_default`.
                // - We are resetting the scheduler; no task will access this buffer.
                unsafe {
                    fpu::FpuState::deallocate(slot.fpu_state);
                }

                slot.fpu_state = ptr::null_mut();

                if !slot.stack_base.is_null() && stacks_to_free.try_reserve(1).is_ok() {
                    stacks_to_free.push((slot.stack_base, slot.stack_size));
                }
            }
        }

        *meta = SchedulerMetadata::new();
        meta.initialized = true;
    });

    // Initialize architecture-visible CR3 tracking via callback backend.
    // This keeps CR3 policy outside of scheduler metadata internals.
    let backend = arch_callbacks();
    let initial_kernel_cr3 = (backend.read_kernel_cr3)();
    set_kernel_and_active_cr3(initial_kernel_cr3);

    #[cfg(debug_assertions)]
    {
        TEST_STOP_REQUESTED.store(false, Ordering::Release);
    }

    // Free old stacks outside the scheduler lock.
    for &(ptr, _size) in &stacks_to_free {
        // SAFETY: Pointers were returned by `allocate_task_stack`.
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        unsafe {
            context::free_task_stack(ptr);
        }
    }

    // Step 1: Drop stale exit-wait registrations from the previous scheduler epoch.
    // This keeps task-id reuse deterministic after `init()` and prevents old
    // waiter IDs from being spuriously unblocked in a fresh run.
    wait::TASK_EXIT_WAITQUEUE.wake_all(|_task_id| {});

    interrupts::register_irq_handler(interrupts::IRQ0_PIT_TIMER_VECTOR, timer_irq_handler);
}

/// Starts scheduling if initialized and at least one task is available.
pub fn start() {
    with_scheduler(|meta| {
        if meta.initialized && !meta.run_queue.is_empty() {
            meta.started = true;
            meta.bootstrap_frame = ptr::null_mut();
            meta.running_slot = None;
            meta.current_queue_pos = meta.run_queue.len() - 1;
        }
    });

    #[cfg(debug_assertions)]
    {
        TEST_STOP_REQUESTED.store(false, Ordering::Release);
    }
}

/// Requests a cooperative scheduler stop on the next timer tick.
#[cfg_attr(not(test), allow(dead_code))]
pub fn request_stop() {
    #[cfg(debug_assertions)]
    {
        TEST_STOP_REQUESTED.store(true, Ordering::Release);
    }
}

/// Replace architecture callback set used by scheduler core.
///
/// This hook keeps round-robin logic independent from concrete MMU/TSS wiring.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_arch_callbacks(callbacks: SchedulerArchCallbacks) {
    *SCHED_ARCH_CALLBACKS.lock() = callbacks;
}

/// Restore default x86_64 backend callbacks (`vmm` + `gdt` integration).
#[cfg_attr(not(test), allow(dead_code))]
pub fn reset_arch_callbacks_to_default() {
    *SCHED_ARCH_CALLBACKS.lock() = SchedulerArchCallbacks::default_callbacks();
}

/// Sets the kernel address-space root used for kernel-task selections.
///
/// `kernel_cr3` must be a valid physical PML4 address for kernel-mode execution.
pub fn set_kernel_address_space_cr3(kernel_cr3: u64) {
    debug_assert!(kernel_cr3 != 0, "kernel_cr3 must be non-zero");
    set_kernel_and_active_cr3(kernel_cr3);
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
    // Snapshot architecture callbacks before taking `SCHED` so the timer path
    // does not nest `SCHED_ARCH_CALLBACKS` under the scheduler lock.
    let callbacks = arch_callbacks();

    // Stacks to free after releasing the scheduler lock.
    // Populated via `core::mem::take` which swaps `pending_free_stacks` with an
    // empty Vec — zero allocation on the take, and deallocation happens outside
    // the lock.  Declared without initialisation: Rust's definite-initialisation
    // analysis ensures the only path that does NOT assign this variable
    // (`!meta.started`) returns before this binding is ever used or dropped.
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut stacks_to_free: Vec<(*mut u8, usize)>;
    let removed_zombie_tasks: bool;

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
        removed_zombie_tasks = reap_zombies(meta, callbacks);

        if meta.run_queue.is_empty() {
            meta.running_slot = None;
            stacks_to_free = take_pending_stacks_for_free(meta, current_frame);
            let frame = bootstrap_or_current(meta, current_frame);
            drop(sched);
            free_pending_stacks(&stacks_to_free);

            if removed_zombie_tasks {
                waitqueue_adapter::wake_all_multi(&wait::TASK_EXIT_WAITQUEUE);
            }

            return frame;
        }

        let detected_slot = find_entry_by_frame(meta, current_frame);

        if detected_slot.is_none() && !frame_within_any_task_stack(meta, current_frame) {
            // Always update bootstrap_frame to the latest non-task frame.
            // This is necessary because the boot stack layout may shift
            // between the initial capture (inside KernelMain) and later
            // ticks (while idling after KernelMain, e.g. blocked in
            // wait_for_task_exit), which would leave bootstrap_frame
            // pointing at a stale IRET frame with corrupted CS/SS values.
            meta.bootstrap_frame = current_frame;
        }

        // Bootstrap frame detection is done; take pending-free stacks for
        // deallocation after the lock is released.  Any stack that still
        // hosts `current_frame` (self-termination via `terminate_task`) is
        // re-queued for the next tick instead of being freed under the CPU.
        stacks_to_free = take_pending_stacks_for_free(meta, current_frame);

        #[cfg(debug_assertions)]
        if TEST_STOP_REQUESTED.swap(false, Ordering::AcqRel) {
            // Collect stacks from all active slots and append to stacks_to_free.
            // pending_free_stacks was already taken above, so only active slots remain.
            for slot in meta.slots.iter() {
                if slot.used && !slot.stack_base.is_null() && stacks_to_free.try_reserve(1).is_ok()
                {
                    stacks_to_free.push((slot.stack_base, slot.stack_size));
                }
            }

            let return_frame = bootstrap_or_current(meta, current_frame);
            reset_scheduler_state(meta);
            drop(sched);
            free_pending_stacks(&stacks_to_free);

            if removed_zombie_tasks {
                waitqueue_adapter::wake_all_multi(&wait::TASK_EXIT_WAITQUEUE);
            }

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
            free_pending_stacks(&stacks_to_free);

            if removed_zombie_tasks {
                waitqueue_adapter::wake_all_multi(&wait::TASK_EXIT_WAITQUEUE);
            }

            return frame;
        }

        select_next_task(meta, detected_slot, current_frame, callbacks)
    };

    // Free stacks from previous tick after releasing the scheduler lock.
    free_pending_stacks(&stacks_to_free);

    if removed_zombie_tasks {
        waitqueue_adapter::wake_all_multi(&wait::TASK_EXIT_WAITQUEUE);
    }

    result
}

/// Lazy FPU state restore — called from the `#NM` (vector 7) exception handler.
///
/// When `CR0.TS` is set by `select_next_task` the next FPU/SSE instruction
/// executed by a task raises `#NM`.  This function:
///
/// 1. Clears `CR0.TS` so `FXRSTOR64` itself does not raise a recursive `#NM`.
/// 2. Defensively saves the previous owner's live FPU registers if ownership
///    was not handed off at switch time (classic lazy-FPU protocol).
/// 3. Restores the current task's saved FPU state via `FXRSTOR64`.
/// 4. Records the task as the new FPU owner so the *next* context switch knows
///    whose state to save.
///
/// After returning the `isr7_nm_stub` executes `iretq`, which re-runs the
/// faulting FPU/SSE instruction — this time successfully.
///
/// If called with no task running (bootstrap / idle context), only `CLTS` is
/// issued; no state is restored.
pub fn handle_fpu_trap() {
    let mut sched = SCHED.lock();
    let meta = &mut *sched;

    // Step 1: Clear CR0.TS so FXRSTOR64 does not raise a recursive #NM.
    // Must happen before FXRSTOR64 (FXRSTOR64 faults if CR0.TS = 1).
    // SAFETY:
    // - This requires `unsafe` because it executes a privileged CPU instruction.
    // - `CLTS` is valid in ring 0 and clears CR0.TS atomically.
    unsafe { fpu::clear_ts() };

    let running_slot = match meta.running_slot {
        Some(slot) => slot,
        None => {
            // #NM fired in the bootstrap/idle context (e.g. inside the hlt loop).
            // CR0.TS is already cleared above; nothing more to do.
            return;
        }
    };

    // Step 2: Defensive save (classic lazy-FPU protocol): if another task
    // still owns the live FPU registers, save them into that task's buffer
    // before the restore below overwrites them.  With the unconditional save
    // in `select_next_task` ownership should always be handed off at switch
    // time, so this is a safety net against any future path that switches
    // contexts without saving — losing a blocked task's FPU state would
    // otherwise be a silent data corruption.
    if let Some(owner) = meta.fpu_owner {
        if owner == running_slot {
            // The live registers already belong to the current task; restoring
            // the saved buffer would overwrite newer live state with stale
            // data.  CR0.TS is cleared, so the retried instruction proceeds.
            return;
        }

        if owner < meta.slots.len() && meta.slots[owner].used {
            let owner_ptr = meta.slots[owner].fpu_state;

            if !owner_ptr.is_null() {
                // SAFETY:
                // - This requires `unsafe` because it executes privileged inline
                //   assembly (FXSAVE64) and dereferences a raw pointer.
                // - `owner_ptr` is a valid, 16-byte-aligned 512-byte buffer.
                // - The owner's FPU state is still live in the CPU registers:
                //   ownership only changes in this function or at FXSAVE time
                //   in `select_next_task`, both of which update `fpu_owner`.
                // - FXSAVE64 does not check CR0.TS (already cleared above).
                unsafe { (*owner_ptr).save() };
            }
        }

        meta.fpu_owner = None;
    }

    // Step 3: Restore the task's saved FPU state.
    let fpu_ptr = meta.slots[running_slot].fpu_state;
    if !fpu_ptr.is_null() {
        // SAFETY:
        // - This requires `unsafe` because it executes privileged inline
        //   assembly (FXRSTOR64) and dereferences a raw pointer.
        // - `fpu_ptr` is a valid, 16-byte-aligned 512-byte buffer holding a
        //   well-formed FXSAVE64 image (written by a prior save or initialised
        //   by `allocate_default`).
        // - CR0.TS has been cleared above, so FXRSTOR64 will not fault.
        unsafe { (*fpu_ptr).restore() };
    }

    // Step 4: Record this task as the FPU owner.
    meta.fpu_owner = Some(running_slot);
}

/// Marks the currently running task as `TaskState::Zombie`.
///
/// The slot remains allocated (`used = true`) so no `spawn_*` call can
/// reuse it.  The scheduler skips zombie tasks during round-robin selection
/// and reaps them at the start of the next `on_timer_tick`, when
/// execution has moved to a different stack.
///
/// # Panics
///
/// Panics if called outside a scheduled task context.
pub fn mark_current_as_zombie() {
    with_scheduler(|meta| {
        let slot = meta
            .running_slot
            .expect("mark_current_as_zombie called outside scheduled task");
        meta.slots[slot].state = TaskState::Zombie;
    });
}

/// Terminates the currently running task and forces an immediate reschedule.
///
/// The task is first marked as `Zombie` so its slot
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
    if let Some(task_id) = current_task_id() {
        crate::io::vfs::close_task_fds(task_id);
    }
    mark_current_as_zombie();
    yield_now();
    loop {
        core::hint::spin_loop();
    }
}

/// Triggers a software timer interrupt to force an immediate reschedule.
///
/// EOI semantics:
/// - `int IRQ0_PIT_TIMER_VECTOR` is a software interrupt, not a physical PIT edge.
/// - The shared IRQ dispatcher currently sends PIC EOI for this vector as part
///   of its normal IRQ epilogue.
/// - Therefore `timer_irq_handler` must never emit its own EOI; doing so would
///   duplicate EOI on real hardware IRQ0 entries and obscure this software path.
pub fn yield_now() {
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Step 1: Trigger software `int` on IRQ0 so we reuse the timer/scheduler IRQ path.
    // - Step 2: Keep EOI handling centralized in `interrupts::dispatch_irq`; this routine only enters the vector.
    // - Valid only in ring 0, which holds for kernel code.
    unsafe {
        asm!(
            "int {vector}",
            vector = const interrupts::IRQ0_PIT_TIMER_VECTOR,
            options(nomem)
        );
    }
}
