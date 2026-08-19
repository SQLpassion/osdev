//! Regression test for the `terminate_task` file-descriptor leak (issue #47).
//!
//! `Fat32OpenFile::owner` (and VFS file-descriptor ownership in general) is
//! recorded as the *packed* task id returned by `scheduler::current_task_id()`
//! -- see `io/fat32.rs`'s `Fat32Fs::open`. `exit_current_task` passes that same
//! packed id straight through to `vfs::close_task_fds`, so a task's normal
//! `Exit` syscall correctly reaps its open descriptors. `terminate_task`, used
//! to forcibly kill a task from the outside, instead extracted the bare slot
//! index first and passed *that* into `close_task_fds`. Since the retain
//! predicate compares against whatever is passed in, the bare slot never
//! matches a packed owner id (any spawned task's generation is >= 1, so its
//! packed id always differs from its bare slot), and the terminated task's
//! open descriptors -- and their backing memory -- were silently leaked
//! forever.
//!
//! This test uses a minimal in-memory `FileSystem` test double (`TrackedFs`,
//! following the `MarkerFs` convention in `vfs_test.rs`) that mirrors
//! `Fat32Fs`'s real ownership bookkeeping (`open` records the caller's packed
//! task id as the owner; `close_task_fds` retains only non-matching owners).
//! This lets the test observe descriptor removal directly and deterministically,
//! without needing a real mounted FAT32 disk or working around the fact that
//! `read`/`close` intentionally return the same `InvalidFd` error for both
//! "descriptor does not exist" and "descriptor belongs to another task".

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use kaos_kernel::arch::interrupts;
use kaos_kernel::io::vfs::{self, FileMode, FileSystem, FsError};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched};
use kaos_kernel::sync::spinlock::SpinLock;

/// `(fd, owner_task_id)` pairs for descriptors "opened" through `TrackedFs`.
///
/// `owner_task_id` is always the *packed* task id of whichever task called
/// `open()`, exactly mirroring `Fat32OpenFile::owner` in `io/fat32.rs`.
static OPEN_FILES: SpinLock<Vec<(usize, usize)>> = SpinLock::new(Vec::new());

/// Monotonic fd allocator for `TrackedFs::open`.
static NEXT_FD: AtomicU64 = AtomicU64::new(1);

/// Set by `victim_task` once it has opened its file and recorded its fd, so
/// the orchestrator knows it is safe to inspect/terminate it.
static VICTIM_READY: AtomicU64 = AtomicU64::new(0);

/// The fd `victim_task` opened, published for the orchestrator to check.
static VICTIM_FD: AtomicU64 = AtomicU64::new(0);

/// Final pass/fail latch consumed by the `#[test_case]` assertion.
static TEST_SUCCESS: AtomicU64 = AtomicU64::new(0);

/// Minimal `FileSystem` test double that tracks fd ownership the same way
/// `Fat32Fs` does, without requiring a mounted disk. Only `open` and
/// `close_task_fds` matter for this test; the remaining methods are
/// unreachable stubs (mirroring the `MarkerFs` double in `vfs_test.rs`).
struct TrackedFs;

impl FileSystem for TrackedFs {
    fn open(&self, _name: &str, _mode: FileMode) -> Result<usize, FsError> {
        // Step 1: Record the *packed* id of whoever is calling, exactly like
        // `Fat32Fs::open` does via `owner: crate::scheduler::current_task_id()`.
        let owner = sched::current_task_id().expect("open must run inside a scheduled task");
        let fd = NEXT_FD.fetch_add(1, Ordering::AcqRel) as usize;
        OPEN_FILES.lock().push((fd, owner));
        Ok(fd)
    }

    fn close(&self, fd: usize) -> Result<(), FsError> {
        // Step 1: Only the owning task may close its own fd (mirrors the
        // fd-ownership contract documented on `FileSystem`).
        let current = sched::current_task_id();
        let mut files = OPEN_FILES.lock();
        match files
            .iter()
            .position(|&(f, owner)| f == fd && current == Some(owner))
        {
            Some(pos) => {
                files.remove(pos);
                Ok(())
            }
            None => Err(FsError::InvalidFd),
        }
    }

    fn read(&self, _fd: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(0)
    }

    fn write(&self, _fd: usize, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::Unsupported)
    }

    fn seek(&self, _fd: usize, _offset: u32) -> Result<(), FsError> {
        Ok(())
    }

    fn eof(&self, _fd: usize) -> Result<bool, FsError> {
        Ok(true)
    }

    fn delete(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported)
    }

    fn read_file(&self, _name: &str) -> Result<Vec<u8>, FsError> {
        Ok(Vec::new())
    }

    fn print_root_directory(&self) {}

    fn close_task_fds(&self, task_id: usize) {
        // Step 1: Retain only descriptors *not* owned by `task_id`, exactly
        // like `Fat32Fs::close_task_fds` (`io/fat32.rs`). This is the exact
        // predicate that silently never matched when `terminate_task` passed
        // a bare slot instead of the packed id `open()` recorded as owner.
        OPEN_FILES.lock().retain(|&(_, owner)| owner != task_id);
    }
}

/// Entry point for the terminate_task fd-leak integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Step 1: Initialize memory and interrupt subsystems required by the scheduler.
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    // Step 2: Mount the in-memory test double; no real disk/AHCI is needed
    // since `TrackedFs` never touches a block device.
    vfs::mount(Box::new(TrackedFs));

    // Step 3: Initialize the scheduler and spawn the orchestrator task, which
    // owns the whole test flow and invokes the standard test harness once it
    // has driven the forced-termination scenario to completion.
    sched::init();
    sched::spawn_kernel_task(orchestrator_task).expect("orchestrator task should spawn");
    sched::start();

    // Step 4: Enable periodic timer interrupts. Unlike a cooperative task,
    // `victim_task` deliberately never yields after signalling readiness, so
    // only preemptive ticks (not explicit `yield_now()` calls) ever take the
    // CPU back from it.
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

/// A task that opens a file and then simply spins forever, holding the
/// descriptor open. It never calls `Exit`/`exit_current_task`; the
/// orchestrator forcibly kills it via `scheduler::terminate_task` instead,
/// which is exactly the code path under test.
extern "C" fn victim_task() -> ! {
    // Step 1: Open a file; `TrackedFs::open` records this task's own packed
    // id (via `current_task_id()`) as the descriptor's owner.
    let fd = vfs::open("whatever.txt", FileMode::Read).expect("victim must open a file");
    VICTIM_FD.store(fd as u64, Ordering::Release);

    // Step 2: Publish readiness so the orchestrator knows the descriptor now
    // exists and it is safe to inspect/terminate this task.
    VICTIM_READY.store(1, Ordering::Release);

    // Step 3: Deliberately never yield or exit. Only the periodic timer (or
    // an external `terminate_task`) can ever take the CPU away from here.
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn orchestrator_task() -> ! {
    // Step 1: Reset state so the test is idempotent if re-run in place.
    OPEN_FILES.lock().clear();
    NEXT_FD.store(1, Ordering::Release);
    VICTIM_READY.store(0, Ordering::Release);
    VICTIM_FD.store(0, Ordering::Release);
    TEST_SUCCESS.store(0, Ordering::Release);

    // Step 2: Spawn the victim task; its packed id is only known to us via
    // the spawn return value, mirroring how a real forced-kill caller would
    // track "the task id I am about to terminate".
    let victim_id = sched::spawn_kernel_task(victim_task).expect("victim task should spawn");

    // Step 3: Yield/preempt until the victim has opened its file and
    // published readiness. Bounded so a scheduling regression fails loudly
    // instead of hanging the whole test binary (the QEMU run is also
    // time-boxed by the test runner).
    let mut ready_spins: u32 = 0;
    while VICTIM_READY.load(Ordering::Acquire) == 0 {
        sched::yield_now();
        ready_spins += 1;
        assert!(
            ready_spins < 64,
            "victim_task never signalled that it had opened its file"
        );
    }
    let victim_fd = VICTIM_FD.load(Ordering::Acquire) as usize;

    // Step 4: Sanity-check the precondition: the victim's descriptor must
    // actually be present, owned by its packed task id (not its bare slot),
    // before we terminate it. This confirms `TrackedFs` is faithfully
    // mirroring `Fat32Fs`'s real ownership bookkeeping.
    assert!(
        OPEN_FILES
            .lock()
            .iter()
            .any(|&(fd, owner)| fd == victim_fd && owner == victim_id),
        "victim's fd must be open and owned by its packed task id before termination"
    );

    // Step 5: Forcibly terminate the victim from the outside -- the exact
    // code path under test, as opposed to a normal self-initiated `Exit`.
    assert!(
        sched::terminate_task(victim_id),
        "orchestrator must be able to terminate the victim task"
    );

    // Step 6: The victim's descriptor must now be gone. Before the fix,
    // `terminate_task` passed the bare slot into `close_task_fds`, which
    // never matched the packed owner id recorded at `open()` time, so this
    // entry would still be present here (a leaked descriptor).
    assert!(
        !OPEN_FILES.lock().iter().any(|&(fd, _)| fd == victim_fd),
        "victim's fd must be removed by terminate_task, not leaked"
    );
    assert!(
        OPEN_FILES.lock().is_empty(),
        "no descriptors should remain once the only task that opened one is terminated"
    );

    TEST_SUCCESS.store(1, Ordering::Release);

    // Step 7: Run the standard test harness from task context so the runner
    // sees the usual "Total/Passed" summary and exits QEMU cleanly.
    test_main();

    // test_main() never returns; it exits QEMU directly.
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: forcibly terminating a task via `scheduler::terminate_task`
/// closes its open file descriptors, the same as a normal `Exit`.
/// Given: A task that opened a file (recorded under its packed task id as
/// owner) and never exits on its own.
/// When: A second task forcibly terminates it via `scheduler::terminate_task`.
/// Then: The victim's file descriptor is removed from the filesystem's open
/// descriptor table -- it must not be leaked, which is what happened when
/// `terminate_task` passed a bare slot index into `close_task_fds` instead of
/// the packed task id that `open()` recorded as the descriptor's owner.
/// Failure Impact: A regression here reintroduces a resource leak: every
/// forced termination of a task with open files would leak those
/// descriptors and, for FAT32, the cached whole-file contents backing them.
#[test_case]
fn test_terminate_task_closes_victims_open_file_descriptors() {
    assert_eq!(
        TEST_SUCCESS.load(Ordering::Acquire),
        1,
        "forced-termination fd-cleanup scenario must complete successfully"
    );
}
