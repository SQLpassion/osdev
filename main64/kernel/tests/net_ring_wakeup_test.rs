//! Genuine two-task "blocked NetRecv is woken by another task's NetSend"
//! integration test for Phase 2 Step 2 (`docs/nic_driver_design.md` §4).
//!
//! This needs real preemptive scheduling, which does not mix with the
//! simpler `set_running_slot_for_test`-driven style used by `net_ring_test.rs`
//! (calling `scheduler::yield_now()` from a call stack that was never itself
//! reached via a genuine context switch is unsound). Structured after
//! `fat32_concurrent_test.rs`: `KernelMain` spawns an orchestrator task and
//! starts the real scheduler; the orchestrator spawns the producer, drives
//! the blocking `NetRecv`, and only then runs the standard `#[test_case]`
//! harness from task context.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::registry;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, SchedulerArchCallbacks};
use kaos_kernel::syscall::{dispatch_checked, SyscallId, SYSCALL_OK};

static TEST_SUCCESS: AtomicU64 = AtomicU64::new(0);

// Parameters the producer task reads once it is scheduled. Set by the
// orchestrator before spawning the producer, so there is no data race (the
// producer cannot run before `sched::spawn_kernel_task` returns).
static PRODUCER_TARGET_TID: AtomicUsize = AtomicUsize::new(0);
static PRODUCER_SENT: AtomicBool = AtomicBool::new(false);

// User pages mapped once in `KernelMain`, before the scheduler starts and
// interrupts are enabled: `map_user_page` asserts interrupts are disabled
// (it must run inside `with_address_space`'s locking discipline), which only
// holds unconditionally before `interrupts::enable()`. Task context, once
// the scheduler is running, is the wrong place to map fresh pages.
static PKT_VA: AtomicU64 = AtomicU64::new(0);
static OUT_VA: AtomicU64 = AtomicU64::new(0);

static TEST_ARCH_KERNEL_CR3: AtomicU64 = AtomicU64::new(0);
static TEST_ARCH_LAST_RSP0: AtomicU64 = AtomicU64::new(0);
static TEST_ARCH_LAST_SWITCH_CR3: AtomicU64 = AtomicU64::new(0);

/// The exact payload the producer sends.
const PAYLOAD: &[u8] = b"woken-by-netsend";

fn test_arch_read_kernel_cr3() -> u64 {
    TEST_ARCH_KERNEL_CR3.load(Ordering::Acquire)
}

fn test_arch_set_kernel_rsp0(rsp0: u64) {
    TEST_ARCH_LAST_RSP0.store(rsp0, Ordering::Release);
}

unsafe fn test_arch_switch_cr3(cr3: u64) {
    TEST_ARCH_LAST_SWITCH_CR3.store(cr3, Ordering::Release);
}

/// Entry point for this integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    // Map both user pages this test needs now, while interrupts are still
    // disabled -- see the doc comment on `PKT_VA`/`OUT_VA` above.
    let pkt_va = vmm::USER_HEAP_BASE + 0x1000;
    let pkt_phys = vmm::page_table::alloc_frame_phys().expect("alloc frame");
    vmm::map_user_page(pkt_va, vmm::page_table::phys_to_pfn(pkt_phys), true)
        .expect("map packet page");
    // SAFETY: `pkt_va` was just mapped writable, one full page, and PAYLOAD
    // is far smaller than a page.
    unsafe {
        core::ptr::copy_nonoverlapping(PAYLOAD.as_ptr(), pkt_va as *mut u8, PAYLOAD.len());
    }
    PKT_VA.store(pkt_va, Ordering::Release);

    let out_va = vmm::USER_HEAP_BASE + 0x2000;
    let out_phys = vmm::page_table::alloc_frame_phys().expect("alloc frame");
    vmm::map_user_page(out_va, vmm::page_table::phys_to_pfn(out_phys), true)
        .expect("map output page");
    OUT_VA.store(out_va, Ordering::Release);

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
    PRODUCER_SENT.store(false, Ordering::Release);
    registry::reset_for_test();

    // Step 1: the orchestrator plays the role of "the driver" -- it is a
    // genuinely running task, so its own packed id is exactly what
    // `scheduler::current_task_id()` will report while it executes the
    // blocking NetRecv call below.
    let own_tid = sched::current_task_id().expect("orchestrator must be the running task");
    registry::register(b"nic:wakeup", own_tid).expect("register driver name");

    // Step 2: configure and spawn the producer. It will push exactly one
    // packet targeting `own_tid` the first time it is scheduled, landing in
    // the TX ring (App -> Driver) since the producer's own tid differs from
    // `own_tid`.
    PRODUCER_TARGET_TID.store(own_tid, Ordering::Release);
    sched::spawn_kernel_task(producer_send_once_then_idle).expect("producer task should spawn");

    // Step 3: call NetRecv on our own tid with a generous bound (2000ms).
    // The ring is empty when this call starts, so it must enter the bounded
    // wait's yield_now() loop -- if that loop genuinely hands the CPU to the
    // producer task, the producer's single NetSend will be observed almost
    // immediately, long before the 2000ms deadline. A regression that turned
    // the wait into a real (uninterruptible) block, or that never yielded to
    // the producer at all, would instead time out here.
    let out_va = OUT_VA.load(Ordering::Acquire);
    let recv_res = dispatch_checked(SyscallId::NET_RECV, own_tid as u64, out_va, 64, 2000);
    let recv_len = recv_res.expect("NetRecv must succeed once the producer sends") as usize;

    // Step 4: verify the exact bytes the producer sent were delivered.
    // (Deliberately not also asserting PRODUCER_SENT here: it is set *after*
    // the producer's NetSend call returns, so a periodic-timer preemption
    // landing in that window can let NetRecv observe the already-queued
    // packet and reach this point before the producer resumes far enough to
    // store the flag -- a race in this flag, not in NetSend/NetRecv
    // themselves. The payload equality check below is race-free: it can
    // only succeed if the producer's send already fully completed, since
    // that is the only source of this exact byte sequence.)
    assert_eq!(recv_len, PAYLOAD.len());
    // SAFETY: `out_va` was mapped writable in `KernelMain` and NetRecv
    // copied `recv_len` bytes into it before returning.
    let received = unsafe { core::slice::from_raw_parts(out_va as *const u8, recv_len) };
    assert_eq!(received, PAYLOAD);

    TEST_SUCCESS.store(1, Ordering::Release);

    // Step 5: run the standard test harness from task context so the runner
    // sees the usual "Total/Passed" summary and exits QEMU cleanly.
    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Sends exactly one packet (read from the `PRODUCER_*` statics) the first
/// time this task is scheduled, then idles forever.
extern "C" fn producer_send_once_then_idle() -> ! {
    loop {
        if !PRODUCER_SENT.load(Ordering::Acquire) {
            let target = PRODUCER_TARGET_TID.load(Ordering::Acquire);
            let va = PKT_VA.load(Ordering::Acquire);
            let send_res = dispatch_checked(
                SyscallId::NET_SEND,
                target as u64,
                va,
                PAYLOAD.len() as u64,
                0,
            );
            assert_eq!(send_res, Ok(SYSCALL_OK));
            PRODUCER_SENT.store(true, Ordering::Release);
        }
        sched::yield_now();
    }
}

/// Contract: a task blocked in `NetRecv` on an empty ring (non-zero
/// `timeout_ms`) is woken and returns the pushed packet once another task
/// calls `NetSend`, without waiting for the full timeout.
/// Given: the orchestrator registered itself as a driver and spawned a
///        producer that sends exactly one packet once scheduled.
/// When: the orchestrator's `NetRecv` call (made from task context, before
///       this assertion runs) is evaluated.
/// Then: it must have returned the producer's exact packet well within the
///       2000ms bound, proving the wait loop's `yield_now()` genuinely
///       handed control to the producer instead of spinning until timeout.
#[test_case]
fn test_net_recv_blocks_then_wakes_when_producer_sends() {
    assert_eq!(
        TEST_SUCCESS.load(Ordering::Acquire),
        1,
        "blocked NetRecv must be woken by the producer's NetSend before the deadline"
    );
}
