//! C2 regression test: #NM must not fire while the SCHED spinlock is held.
//!
//! The scheduler uses lazy FPU switching: `select_next_task` sets `CR0.TS = 1`
//! at the end of every context switch.  The next timer tick enters
//! `on_timer_tick` and acquires the non-reentrant `SCHED` spinlock.  Because
//! `cli` masks maskable interrupts but *not* the `#NM` exception, any SSE
//! instruction executed inside that critical section with `TS = 1` would raise
//! `#NM`, whose handler tries to re-acquire `SCHED` and deadlocks on a single
//! core.
//!
//! This test enables the scheduler's test hook that asserts `CR0.TS = 0` while
//! the scheduler lock is held, then drives `on_timer_tick` manually.  Without
//! the C2 mitigation the assertion fires (TS is still 1 from the previous
//! switch); with the mitigation `clear_ts()` runs before the lock is acquired.
//!
//! A second test below exercises the same C2 invariant at `with_scheduler` —
//! the choke-point used by ~20 other scheduler call sites (`block_task`,
//! `unblock_task`, `terminate_task`, `spawn_internal`, every `api.rs`
//! accessor, ...) which historically did *not* clear `CR0.TS` at all (see
//! issue #45). That gap was inert only because the kernel's build target
//! disables SSE, so no compiler-emitted instruction could trigger `#NM`
//! inside those closures — a fragile, non-structural guarantee that this
//! test makes an explicit, checked contract instead.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use kaos_kernel::arch::fpu;
use kaos_kernel::arch::interrupts::{self, SavedRegisters};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, TEST_SCHEDULER_ENTER_ASSERT_TS_CLEAR};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

extern "C" fn dummy_task() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: scheduler critical section enters with CR0.TS clear.
/// Given: The scheduler is initialized and one kernel task has been spawned.
/// When: The test hook is enabled and a timer tick is driven through the scheduler.
/// Then: CR0.TS must be clear while the SCHED lock is held, preventing an #NM deadlock.
/// Failure Impact: Regression of C2 — compiler-emitted SSE inside the scheduler critical
/// section could re-enter the non-reentrant SCHED spinlock and hang the system.
#[test_case]
fn test_scheduler_critical_section_enters_with_ts_clear() {
    sched::init();

    let _task = sched::spawn_kernel_task(dummy_task).expect("task should spawn");

    sched::start();

    // Enable the invariant check *before* entering the scheduler critical section.
    TEST_SCHEDULER_ENTER_ASSERT_TS_CLEAR.store(true, Ordering::Release);

    let mut bootstrap = SavedRegisters::default();
    let bootstrap_ptr = &mut bootstrap as *mut SavedRegisters;

    // First timer tick selects the spawned task and re-arms CR0.TS = 1 at the
    // end of `select_next_task`.  The hook asserts TS = 0 while SCHED is held,
    // which holds here because TS has not been armed yet.
    let current = sched::on_timer_tick(bootstrap_ptr);

    // Second timer tick: the lazy-FPU bit is now armed (TS = 1).  Without the
    // C2 mitigation the hook would observe TS = 1 inside the SCHED critical
    // section and panic, because any SSE instruction there could raise #NM and
    // re-enter the non-reentrant lock.  With the mitigation `clear_ts()` runs
    // before the lock is acquired, so the assertion passes.
    let _ = sched::on_timer_tick(current);

    // Disable the hook again so later tests in this binary are not affected.
    TEST_SCHEDULER_ENTER_ASSERT_TS_CLEAR.store(false, Ordering::Release);
}

/// Contract: `with_scheduler` — the choke-point behind ~20 scheduler call sites
/// other than `on_timer_tick`/`handle_fpu_trap` — also enters the `SCHED`
/// critical section with `CR0.TS` clear (issue #45), and restores `CR0.TS` to
/// its pre-call armed state once `SCHED` is released.
///
/// Given: `CR0.TS` is deliberately left armed, exactly as `select_next_task`
///        leaves it at the end of a real context switch (the running task has
///        not yet trapped `#NM`, so it does not own live FPU state yet).
/// When:  A `with_scheduler`-guarded API (`is_running`) is invoked directly,
///        without going through `on_timer_tick` first.
/// Then:  The test hook must observe `CR0.TS = 0` while `SCHED` is held, and
///        `CR0.TS` must be **re-armed** (1) once the call returns — restoring
///        the pre-call state rather than leaving it permanently cleared.
/// Failure Impact: Before issue #45's original fix, `with_scheduler` never
/// touched `CR0.TS` at all, so every one of its ~20 call sites ran its
/// critical section with whatever `CR0.TS` value a prior context switch left
/// behind — inert only as long as the build target disables SSE. A later
/// revision of that fix cleared `CR0.TS` on entry but never restored it,
/// which permanently defeated the lazy-FPU trap for any task that called a
/// `with_scheduler`-guarded API before its first FPU/SSE instruction: that
/// instruction would then silently execute on stale/foreign register content
/// instead of trapping into `handle_fpu_trap` to restore its own state. This
/// test pins the corrected contract: clear only for the duration of the
/// locked section, restored immediately after.
#[test_case]
fn test_with_scheduler_entry_points_enter_with_ts_clear_and_restore_it() {
    sched::init();

    // Simulate the post-context-switch state `select_next_task` leaves behind:
    // `CR0.TS = 1`, with no intervening `on_timer_tick` call to clear it.
    //
    // SAFETY:
    // - `set_ts` is a ring-0-only instruction; this test binary runs entirely
    //   in ring 0 (it is the kernel itself, boots directly into `KernelMain`).
    // - No FPU/SSE instruction is executed between this call and the
    //   `with_scheduler` call below, so no spurious `#NM` can fire in between.
    unsafe { fpu::set_ts() };

    // Enable the invariant check *before* entering the with_scheduler-guarded
    // critical section, mirroring the on_timer_tick test above.
    TEST_SCHEDULER_ENTER_ASSERT_TS_CLEAR.store(true, Ordering::Release);

    // `is_running` is one of the ~20 call sites named in issue #45 that goes
    // through `with_scheduler` and previously never touched `CR0.TS` at all.
    // Without the fix, the hook inside `with_scheduler` would observe
    // `CR0.TS = 1` here and panic; with the fix, `clear_ts()` runs before
    // `SCHED.lock()`, so the assertion passes.
    let _ = sched::is_running();

    // Disable the hook again so later tests in this binary are not affected.
    TEST_SCHEDULER_ENTER_ASSERT_TS_CLEAR.store(false, Ordering::Release);

    // The call must have *re-armed* CR0.TS once it returns, since it was
    // armed (not yet trapped) on entry. Leaving it clear here would mean the
    // running task's own FPU state is never restored via `handle_fpu_trap`.
    //
    // SAFETY:
    // - `read_ts` only reads CR0, which is safe in ring 0.
    unsafe {
        assert!(
            fpu::read_ts(),
            "with_scheduler must re-arm CR0.TS after returning if it was armed on entry"
        );
    }
}

/// Contract: `with_scheduler` must NOT arm `CR0.TS` on return if it was
/// already clear on entry (the running task already owns live FPU state).
///
/// Given: `CR0.TS` is clear, as it would be for a task that has already
///        trapped `#NM` once and had its state restored by `handle_fpu_trap`.
/// When:  A `with_scheduler`-guarded API (`is_running`) is invoked.
/// Then:  `CR0.TS` must remain clear once the call returns — `with_scheduler`
///        must restore the pre-call state, not unconditionally arm it.
/// Failure Impact: If `with_scheduler` armed `CR0.TS` unconditionally on
/// return, a task that already owns live FPU state would take a spurious
/// `#NM` trap on its next FPU/SSE instruction, needlessly re-running the
/// FXRSTOR64 path against state that was already correctly in place.
#[test_case]
fn test_with_scheduler_leaves_ts_clear_if_already_clear_on_entry() {
    sched::init();

    // Simulate a task that already owns live FPU state (already trapped
    // #NM once): CR0.TS = 0.
    //
    // SAFETY:
    // - `clear_ts` is a ring-0-only instruction; this test binary runs
    //   entirely in ring 0.
    unsafe { fpu::clear_ts() };

    let _ = sched::is_running();

    // SAFETY:
    // - `read_ts` only reads CR0, which is safe in ring 0.
    unsafe {
        assert!(
            !fpu::read_ts(),
            "with_scheduler must not arm CR0.TS if it was already clear on entry"
        );
    }
}
