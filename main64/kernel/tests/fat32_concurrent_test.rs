//! FAT32 concurrent file I/O test.
//!
//! Verifies R-03: no filesystem or block-device spinlock is held across blocking
//! disk I/O. Two kernel tasks read files in parallel; if any lock were held
//! across a yielding ATA wait, the second task would spin with interrupts
//! disabled and the kernel would hang.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, SchedulerArchCallbacks};

static TASK_A_DONE: AtomicU64 = AtomicU64::new(0);
static TASK_B_DONE: AtomicU64 = AtomicU64::new(0);
static TEST_SUCCESS: AtomicU64 = AtomicU64::new(0);

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

/// Entry point for the concurrent I/O integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Step 1: Initialize memory and interrupt subsystems required by the scheduler and heap.
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    // Step 2: Initialize ATA PIO and select it as the active block device.
    kaos_kernel::drivers::ata::init();
    kaos_kernel::drivers::block::init_ata();

    // Step 3: Mount the FAT32 filesystem (superfloppy VBR at LBA 0).
    let vol = kaos_kernel::io::fat32::Fat32Volume::mount(0).expect("FAT32 must mount at LBA 0");
    kaos_kernel::io::vfs::mount(Box::new(kaos_kernel::io::fat32::Fat32Fs::new(vol)));

    // Step 4: Configure scheduler architecture callbacks for this test epoch.
    // We use the real kernel PML4 address so CR3 bookkeeping stays consistent;
    // the callbacks themselves only record values for test diagnostics.
    TEST_ARCH_KERNEL_CR3.store(vmm::get_pml4_address(), Ordering::Release);
    TEST_ARCH_LAST_RSP0.store(0, Ordering::Release);
    TEST_ARCH_LAST_SWITCH_CR3.store(0, Ordering::Release);

    sched::set_arch_callbacks(SchedulerArchCallbacks {
        read_kernel_cr3: test_arch_read_kernel_cr3,
        set_kernel_rsp0: test_arch_set_kernel_rsp0,
        switch_cr3: test_arch_switch_cr3,
    });

    // Step 5: Reset and initialize the scheduler, spawn the test orchestrator,
    // and start preemptive scheduling. The bootstrap context is intentionally
    // left out of the scheduler; once started, the orchestrator task owns the
    // test flow and invokes the standard test harness from task context.
    sched::init();
    sched::spawn_kernel_task(orchestrator_task).expect("orchestrator task should spawn");
    sched::start();

    // Step 6: Enable periodic timer interrupts so the scheduler can preempt
    // between the reader tasks. After this point control never returns here;
    // the orchestrator task exits QEMU via the test harness.
    interrupts::init_periodic_timer(250);
    interrupts::enable();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

extern "C" fn orchestrator_task() -> ! {
    // Step 1: Reset state flags so the test is idempotent if re-run in place.
    TASK_A_DONE.store(0, Ordering::Release);
    TASK_B_DONE.store(0, Ordering::Release);
    TEST_SUCCESS.store(0, Ordering::Release);

    // Step 2: Spawn two kernel tasks that perform file I/O in parallel.
    let task_a = sched::spawn_kernel_task(concurrent_reader_task_a).expect("task A should spawn");
    let task_b = sched::spawn_kernel_task(concurrent_reader_task_b).expect("task B should spawn");

    // Step 3: Wait cooperatively until both reader tasks have exited.
    // These calls yield the orchestrator so the readers can actually run.
    sched::wait_for_task_exit(task_a);
    sched::wait_for_task_exit(task_b);

    // Step 4: Verify both tasks reported successful reads and flag success.
    let a_done = TASK_A_DONE.load(Ordering::Acquire) == 1;
    let b_done = TASK_B_DONE.load(Ordering::Acquire) == 1;
    assert!(a_done, "task A must have completed its read");
    assert!(b_done, "task B must have completed its read");
    TEST_SUCCESS.store(1, Ordering::Release);

    // Step 5: Run the standard test harness from task context so the runner
    // sees the usual "Total/Passed" summary and exits QEMU cleanly.
    test_main();

    // test_main() never returns; it exits QEMU directly.
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn concurrent_reader_task_a() -> ! {
    // Step 1: Read a real file from the FAT32 image under the scheduler.
    // If the mount or descriptor lock were held across the block-device wait,
    // another task spinning on the same lock would prevent the timer IRQ from
    // rescheduling this task back in, causing a permanent hang.
    let data = kaos_kernel::io::vfs::read_file("HELLO.BIN")
        .expect("task A must read HELLO.BIN successfully");
    assert!(!data.is_empty(), "task A read HELLO.BIN but it was empty");

    // Step 2: Record success and yield so task B gets a scheduling slot.
    TASK_A_DONE.store(1, Ordering::Release);
    sched::yield_now();

    // Step 3: Exit; the orchestrator task observes this via the scheduler.
    sched::exit_current_task();
}

extern "C" fn concurrent_reader_task_b() -> ! {
    // Step 1: Read a different file while task A is also active.
    let data = kaos_kernel::io::vfs::read_file("READLINE.BIN")
        .expect("task B must read READLINE.BIN successfully");
    assert!(
        !data.is_empty(),
        "task B read READLINE.BIN but it was empty"
    );

    // Step 2: Record success and yield back to the orchestrator.
    TASK_B_DONE.store(1, Ordering::Release);
    sched::yield_now();

    // Step 3: Exit.
    sched::exit_current_task();
}

/// Contract: concurrent file reads from two scheduler tasks complete without deadlock.
/// Given: FAT32 is mounted and the orchestrator task has already waited for both readers.
/// When: This assertion is evaluated in the orchestrator's task context.
/// Then: Both reads succeeded and the kernel did not hang.
#[test_case]
fn test_concurrent_file_reads_do_not_deadlock() {
    assert_eq!(
        TEST_SUCCESS.load(Ordering::Acquire),
        1,
        "concurrent reader tasks must both complete their file reads"
    );
}

#[test_case]
fn test_monotonic_fd_allocation() {
    use kaos_kernel::io::vfs::FileMode;
    let fd1 = kaos_kernel::io::vfs::open("HELLO.BIN", FileMode::Read).expect("open 1");
    kaos_kernel::io::vfs::close(fd1).expect("close 1");

    let fd2 = kaos_kernel::io::vfs::open("HELLO.BIN", FileMode::Read).expect("open 2");
    kaos_kernel::io::vfs::close(fd2).expect("close 2");

    assert!(
        fd2 > fd1,
        "file descriptors must be monotonic, fd2 {} <= fd1 {}",
        fd2,
        fd1
    );
}

#[test_case]
fn test_fd_isolation_between_tasks() {
    use kaos_kernel::io::vfs::{FileMode, FsError};

    static ISOLATION_FD: AtomicU64 = AtomicU64::new(0);
    static ISOLATION_TASK_DONE: AtomicU64 = AtomicU64::new(0);

    // 1. Open a file in the orchestrator task.
    let fd = kaos_kernel::io::vfs::open("HELLO.BIN", FileMode::Read).expect("open in orchestrator");
    ISOLATION_FD.store(fd as u64, Ordering::Release);

    // 2. Spawn a child task that tries to access this FD.
    extern "C" fn evil_child_task() -> ! {
        let target_fd = ISOLATION_FD.load(Ordering::Acquire) as usize;
        let mut buf = [0u8; 10];

        let res_read = kaos_kernel::io::vfs::read(target_fd, &mut buf);
        assert!(
            matches!(res_read, Err(FsError::InvalidFd)),
            "child should be denied read access to parent's fd"
        );

        let res_close = kaos_kernel::io::vfs::close(target_fd);
        assert!(
            matches!(res_close, Err(FsError::InvalidFd)),
            "child should be denied closing parent's fd"
        );

        ISOLATION_TASK_DONE.store(1, Ordering::Release);
        sched::exit_current_task();
    }

    let child = sched::spawn_kernel_task(evil_child_task).expect("spawn evil child");
    sched::wait_for_task_exit(child);

    assert_eq!(
        ISOLATION_TASK_DONE.load(Ordering::Acquire),
        1,
        "evil child must complete"
    );

    // 3. Orchestrator can still read and close its own FD.
    let mut buf = [0u8; 10];
    let res = kaos_kernel::io::vfs::read(fd, &mut buf);
    assert!(
        res.is_ok(),
        "orchestrator must still be able to read its own fd"
    );

    kaos_kernel::io::vfs::close(fd).expect("orchestrator close");
}
