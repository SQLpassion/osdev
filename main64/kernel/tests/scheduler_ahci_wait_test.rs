//! Integration test for the AHCI request-slot cooperative wait path (issue #85).
//!
//! Asserts that a task waiting for the AHCI transfer slot actually blocks
//! cooperatively (via the scheduler) and is woken when the slot is released.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::ahci;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, TaskState};

static TASK1_READY: AtomicU64 = AtomicU64::new(0);
static TASK1_DONE: AtomicU64 = AtomicU64::new(0);
static TASK2_BLOCKED_OBSERVED: AtomicU64 = AtomicU64::new(0);
static TASK2_DONE: AtomicU64 = AtomicU64::new(0);
static TEST_SUCCESS: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

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

extern "C" fn task1() -> ! {
    let guard = ahci::acquire_transfer_slot_for_test_blocking();
    TASK1_READY.store(1, Ordering::Release);

    // Hold it until orchestrator observes task2 is blocked
    while TASK2_BLOCKED_OBSERVED.load(Ordering::Acquire) == 0 {
        sched::yield_now();
    }

    core::mem::drop(guard);
    TASK1_DONE.store(1, Ordering::Release);

    loop {
        sched::yield_now();
    }
}

extern "C" fn task2() -> ! {
    // Wait until task1 holds the lock
    while TASK1_READY.load(Ordering::Acquire) == 0 {
        sched::yield_now();
    }

    // This should block cooperatively!
    let guard = ahci::acquire_transfer_slot_for_test_blocking();

    core::mem::drop(guard);
    TASK2_DONE.store(1, Ordering::Release);

    loop {
        sched::yield_now();
    }
}

extern "C" fn orchestrator_task() -> ! {
    let _t1_id = sched::spawn_kernel_task(task1).expect("task1 should spawn");
    let t2_id = sched::spawn_kernel_task(task2).expect("task2 should spawn");

    // Wait for task1 to acquire the lock
    while TASK1_READY.load(Ordering::Acquire) == 0 {
        sched::yield_now();
    }

    // Now task2 should eventually block waiting for it
    let mut spins = 0;
    while sched::task_state(t2_id) != Some(TaskState::Blocked) {
        sched::yield_now();
        spins += 1;
        if spins > 100_000 {
            panic!("task2 did not block on AHCI waitqueue");
        }
    }

    TASK2_BLOCKED_OBSERVED.store(1, Ordering::Release);

    // Wait for task2 to finish (which means it woke up)
    spins = 0;
    while TASK2_DONE.load(Ordering::Acquire) == 0 {
        sched::yield_now();
        spins += 1;
        if spins > 100_000 {
            panic!("task2 did not complete after being blocked");
        }
    }

    TEST_SUCCESS.store(1, Ordering::Release);
    test_main();
    loop {
        sched::yield_now();
    }
}

#[test_case]
fn test_ahci_request_slot_cooperative_wait() {
    assert_eq!(
        TEST_SUCCESS.load(Ordering::Acquire),
        1,
        "cooperative wait test must complete successfully"
    );
}
