//! Regression test for a live QEMU bug report: after the shell's `load
//! <name.drv>` command starts a background NIC driver, the shell's `>`
//! prompt never reappears and `exit` stops working.
//!
//! `run_background_driver` (Phase 2 Step 6, `lib_driver_runtime/src/repl.rs`)
//! is the first place in this codebase where a Ring-3 task is genuinely
//! blocked on keyboard input (`GetChar` -> `keyboard::read_char_blocking`,
//! which uses the kernel-internal, nested `scheduler::yield_now()` to
//! deschedule) while a second Ring-3 task runs concurrently, tight-looping
//! on `process::yield_now()` every iteration with no pacing. This test
//! reproduces the concurrency shape (not the exact `int 0x80` framing, which
//! kernel test tasks never go through -- see `net_ring_wakeup_test.rs`'s doc
//! comment for why `sched::yield_now()` is the right stand-in here) with a
//! real, running scheduler: a task genuinely blocked in the `GetChar`
//! syscall, an always-ready task that never stops yielding, and the real
//! `keyboard_worker_task` decode pipeline in between.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::{keyboard, time};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, SchedulerArchCallbacks};
use kaos_kernel::syscall::{dispatch_checked, SyscallId};

static TEST_SUCCESS: AtomicU64 = AtomicU64::new(0);
static GOT_CHAR: AtomicU8 = AtomicU8::new(0);
static DRIVER_KEEP_RUNNING: AtomicBool = AtomicBool::new(true);

static TEST_ARCH_KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
static TEST_ARCH_LAST_RSP0: AtomicU64 = AtomicU64::new(0);
static TEST_ARCH_LAST_SWITCH_CR3: AtomicU64 = AtomicU64::new(0);

fn test_arch_read_kernel_cr3() -> u64 {
    TEST_ARCH_KERNEL_CR3.load(Ordering::Acquire)
}

fn test_arch_set_kernel_rsp0(rsp0: u64) {
    TEST_ARCH_LAST_RSP0.store(rsp0, Ordering::Release);
}

unsafe fn test_arch_switch_cr3(cr3: u64) {
    TEST_ARCH_LAST_SWITCH_CR3.store(cr3, Ordering::Release);
}

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);
    keyboard::init();

    TEST_ARCH_KERNEL_CR3.store(vmm::get_pml4_address(), Ordering::Release);
    sched::set_arch_callbacks(SchedulerArchCallbacks {
        read_kernel_cr3: test_arch_read_kernel_cr3,
        set_kernel_rsp0: test_arch_set_kernel_rsp0,
        switch_cr3: test_arch_switch_cr3,
    });

    sched::init();
    sched::spawn_kernel_task(orchestrator_task).expect("orchestrator task should spawn");
    sched::start();

    interrupts::init_periodic_timer(250);
    interrupts::enable();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

extern "C" fn orchestrator_task() -> ! {
    TEST_SUCCESS.store(0, Ordering::Release);
    GOT_CHAR.store(0, Ordering::Release);
    DRIVER_KEEP_RUNNING.store(true, Ordering::Release);

    // Step 1: spawn the real keyboard bottom-half worker (raw scancode ->
    // decoded char, exactly as at real boot), the GetChar-blocked "shell",
    // and the always-ready "background driver" stand-in, in that order.
    sched::spawn_kernel_task(keyboard::keyboard_worker_task)
        .expect("keyboard worker task should spawn");
    sched::spawn_kernel_task(shell_getchar_task).expect("shell task should spawn");
    sched::spawn_kernel_task(busy_driver_task).expect("driver task should spawn");

    // Step 2: give every task a chance to reach its first blocking point
    // (shell_getchar_task's GetChar call) before injecting the keystroke.
    for _ in 0..50 {
        sched::yield_now();
    }

    // Step 3: inject one raw scancode -- make code for 'a' (0x1e), matching
    // `keyboard_e2e_test.rs`'s convention.
    keyboard::enqueue_raw_scancode(0x1e);

    // Step 4: bounded wait. A task blocked in GetChar must be woken and
    // return the injected character well within this deadline, even while
    // `busy_driver_task` never stops calling `yield_now()`. A regression
    // that starves or hangs the GetChar-blocked task under a concurrently
    // runnable, always-ready task times out here instead of succeeding.
    let ticks_per_ms = time::tsc_ticks_per_us().saturating_mul(1000);
    let deadline = time::rdtsc().saturating_add(ticks_per_ms.saturating_mul(2000));
    while time::rdtsc() < deadline {
        if GOT_CHAR.load(Ordering::Acquire) != 0 {
            break;
        }
        sched::yield_now();
    }

    DRIVER_KEEP_RUNNING.store(false, Ordering::Release);

    if GOT_CHAR.load(Ordering::Acquire) == b'a' {
        TEST_SUCCESS.store(1, Ordering::Release);
    }

    // Step 5: run the standard test harness from task context so the runner
    // sees the usual "Total/Passed" summary and exits QEMU cleanly.
    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Stand-in for the shell: makes one real, blocking `GetChar` syscall (the
/// exact syscall `console::readline` relies on) and records the result.
extern "C" fn shell_getchar_task() -> ! {
    if let Ok(ch) = dispatch_checked(SyscallId::GET_CHAR, 0, 0, 0, 0) {
        GOT_CHAR.store(ch as u8, Ordering::Release);
    }

    loop {
        sched::yield_now();
    }
}

/// Stand-in for `run_background_driver`: an always-ready task that never
/// blocks and never stops yielding, exactly like the real driver's
/// drain-TX/poll-RX/publish-status/yield loop once its rings are empty and
/// the polled device has nothing pending.
extern "C" fn busy_driver_task() -> ! {
    while DRIVER_KEEP_RUNNING.load(Ordering::Acquire) {
        sched::yield_now();
    }

    loop {
        sched::yield_now();
    }
}

/// Contract: a task blocked in `GetChar` must still be woken and return the
/// injected character even while another always-ready task keeps calling
/// `yield_now()` on every iteration.
/// Given: a real running scheduler with the keyboard worker task, a
///        GetChar-blocked "shell" task, and an always-ready "driver" task.
/// When: a raw scancode for 'a' is injected after all three tasks have had a
///       chance to reach their steady state.
/// Then: the "shell" task's GetChar call must return `b'a'` well within the
///       bounded wait, proving the background task's constant yielding does
///       not starve or hang the blocked consumer.
#[test_case]
fn test_getchar_completes_with_busy_background_task() {
    assert_eq!(
        TEST_SUCCESS.load(Ordering::Acquire),
        1,
        "a task blocked in GetChar must be woken with the injected character \
         even while another always-ready task keeps calling yield_now()"
    );
}
