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

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

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
