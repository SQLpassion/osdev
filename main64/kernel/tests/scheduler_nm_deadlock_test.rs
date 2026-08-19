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
/// critical section with `CR0.TS` clear (issue #45).
///
/// Given: `CR0.TS` is deliberately left armed, exactly as `select_next_task`
///        leaves it at the end of a real context switch.
/// When:  A `with_scheduler`-guarded API (`is_running`) is invoked directly,
///        without going through `on_timer_tick` first.
/// Then:  The test hook must observe `CR0.TS = 0` while `SCHED` is held, and
///        `CR0.TS` must still be 0 once the call returns.
/// Failure Impact: Before this fix, `with_scheduler` never touched `CR0.TS`,
/// so every one of its ~20 call sites ran its critical section with whatever
/// `CR0.TS` value a prior context switch left behind. That is inert only as
/// long as the build target disables SSE (see issue #45) — re-enabling SSE, or
/// a future dependency shipping SIMD code, would turn any such call site into
/// a silent `#NM` re-entrant-lock deadlock.
#[test_case]
fn test_with_scheduler_entry_points_enter_with_ts_clear() {
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

    // The call must also have left CR0.TS clear as an observable side effect,
    // independent of the debug-only assertion hook above.
    //
    // SAFETY:
    // - `read_ts` only reads CR0, which is safe in ring 0.
    unsafe {
        assert!(
            !fpu::read_ts(),
            "with_scheduler must leave CR0.TS clear after returning"
        );
    }
}
