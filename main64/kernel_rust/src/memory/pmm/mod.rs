//! Physical memory manager (PMM) for allocating and freeing 4 KiB page frames.
//!
//! Design summary:
//! - Bitmap-backed allocator tracking available physical page frames.
//! - Queries usable memory regions from BIOS Information Block (BIB) and BIOS Memory Map.
//! - Reserves kernel code/data, bootloader stack, and allocator bitmap regions to prevent overwriting.
//! - Thread-safe access synchronized via a global spinlock (`GlobalPmm`).
//! - Tracks allocation states using 1 bit per page frame (4 KiB page frame granularity).
//!
//! Layout in memory:
//! - Kernel loaded at `KERNEL_OFFSET` (1 MiB).
//! - PMM layout structures (including headers and bitmaps) placed immediately after the kernel BSS section, aligned to 4 KiB page boundaries.
//! - Bitmaps for each memory region follow the PmmRegion structures array sequentially.
//!
//! Notes:
//! - `0` in a bitmap represents a free page frame, while `1` represents an allocated/reserved frame.
//! - Consecutive allocations search from the first available region to minimize fragmentation.

use crate::drivers::screen::with_screen;
use crate::sync::spinlock::SpinLock;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

pub mod types;
pub mod manager;

#[allow(unused_imports)]
pub use types::{
    align_up, virt_to_phys, PageFrame, PmmLayoutHeader, PmmRegion, KERNEL_OFFSET, PAGE_SIZE, STACK_TOP,
};
pub use manager::PhysicalMemoryManager;

/// Wrapper that holds the global PMM behind a `SpinLock` for thread-safe access.
/// An `AtomicBool` tracks whether `init()` has been called.
struct GlobalPmm {
    inner: SpinLock<PhysicalMemoryManager>,
    initialized: AtomicBool,
    debug_enabled: AtomicBool,
}

impl GlobalPmm {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(PhysicalMemoryManager {
                header: core::ptr::null_mut(),
            }),
            initialized: AtomicBool::new(false),
            debug_enabled: AtomicBool::new(false),
        }
    }
}

static PMM: GlobalPmm = GlobalPmm::new();

#[inline]
fn debug_enabled() -> bool {
    PMM.debug_enabled.load(Ordering::Acquire)
}

/// Initializes the global physical memory manager.
///
/// `debug_output` controls whether PMM allocation/free events are logged.
pub fn init(debug_output: bool) {
    {
        let mut pmm = PMM.inner.lock();
        *pmm = PhysicalMemoryManager::new();
    }
    PMM.debug_enabled.store(debug_output, Ordering::Release);
    PMM.initialized.store(true, Ordering::Release);
}

/// Returns whether the global PMM has been initialized.
#[inline]
pub fn is_initialized() -> bool {
    PMM.initialized.load(Ordering::Acquire)
}

/// Executes a closure with a mutable reference to the PMM instance.
///
/// This function is thread-safe: it acquires a spinlock that disables
/// interrupts while the closure executes, preventing preemption.
pub fn with_pmm<R>(f: impl FnOnce(&mut PhysicalMemoryManager) -> R) -> R {
    debug_assert!(
        PMM.initialized.load(Ordering::Acquire),
        "PMM not initialized"
    );
    let mut guard = PMM.inner.lock();
    f(&mut guard)
}

/// Returns the current number of free physical frames across all PMM regions.
pub fn free_frame_count() -> u64 {
    with_pmm(|mgr| mgr.total_free_frames())
}

#[inline]
pub(crate) fn log_alloc(pfn: u64, region_index: u32) {
    if !debug_enabled() {
        return;
    }
    crate::logging::logln(
        "pmm",
        format_args!(
            "PMM: allocated frame pfn=0x{:x} phys=0x{:x} region={}",
            pfn,
            pfn * PAGE_SIZE,
            region_index
        ),
    );
}

#[inline]
pub(crate) fn log_release(pfn: u64, region_index: u32) {
    if !debug_enabled() {
        return;
    }
    crate::logging::logln(
        "pmm",
        format_args!(
            "PMM: released frame pfn=0x{:x} phys=0x{:x} region={}",
            pfn,
            pfn * PAGE_SIZE,
            region_index
        ),
    );
}

/// Runs PMM runtime self-tests and prints results to the screen.
///
/// The test deliberately avoids one long PMM critical section:
/// each alloc/release operation acquires the PMM lock independently so IRQs
/// are not blocked for the whole stress run.
pub fn run_self_test(stress_iters: u32) {
    fn print_test_line(args: core::fmt::Arguments<'_>) {
        with_screen(|screen| {
            let _ = screen.write_fmt(args);
            let _ = writeln!(screen);
        });
    }

    fn alloc_test_frame() -> Option<PageFrame> {
        with_pmm(|mgr| mgr.alloc_frame())
    }

    fn release_test_pfn(pfn: u64) -> bool {
        with_pmm(|mgr| mgr.release_pfn(pfn))
    }

    let mut failures = 0u32;
    crate::logging::logln(
        "pmm",
        format_args!("[pmm-test] start (stress={})", stress_iters),
    );

    print_test_line(format_args!(
        "Running PMM self-test (stress: {})...",
        stress_iters
    ));

    // Step 1: perform deterministic single-frame checks.
    let frame0 = match alloc_test_frame() {
        Some(f) => f,
        None => {
            crate::logging::logln("pmm", format_args!("[pmm-test] FAIL alloc frame0"));
            print_test_line(format_args!("  [FAIL] alloc frame0"));
            return;
        }
    };

    let frame1 = match alloc_test_frame() {
        Some(f) => f,
        None => {
            crate::logging::logln("pmm", format_args!("[pmm-test] FAIL alloc frame1"));
            print_test_line(format_args!("  [FAIL] alloc frame1"));
            let _ = release_test_pfn(frame0.pfn);

            return;
        }
    };

    let frame2 = match alloc_test_frame() {
        Some(f) => f,
        None => {
            crate::logging::logln("pmm", format_args!("[pmm-test] FAIL alloc frame2"));
            print_test_line(format_args!("  [FAIL] alloc frame2"));
            let _ = release_test_pfn(frame1.pfn);
            let _ = release_test_pfn(frame0.pfn);

            return;
        }
    };

    crate::logging::logln(
        "pmm",
        format_args!(
            "[pmm-test] allocated pfns: {}, {}, {}",
            frame0.pfn, frame1.pfn, frame2.pfn
        ),
    );

    if frame0.pfn == frame1.pfn || frame1.pfn == frame2.pfn || frame0.pfn == frame2.pfn {
        failures += 1;
        crate::logging::logln("pmm", format_args!("[pmm-test] FAIL unique PFNs"));
        print_test_line(format_args!("  [FAIL] allocated PFNs are not unique"));
    } else {
        crate::logging::logln("pmm", format_args!("[pmm-test] OK unique PFNs"));
        print_test_line(format_args!(
            "  [ OK ] unique PFNs on consecutive allocations"
        ));
    }

    let addr0 = frame0.physical_address();
    let addr1 = frame1.physical_address();
    let addr2 = frame2.physical_address();

    if addr0 % PAGE_SIZE != 0 || addr1 % PAGE_SIZE != 0 || addr2 % PAGE_SIZE != 0 {
        failures += 1;
        crate::logging::logln(
            "pmm",
            format_args!(
                "[pmm-test] FAIL alignment: {:#x}, {:#x}, {:#x}",
                addr0, addr1, addr2
            ),
        );

        print_test_line(format_args!("  [FAIL] physical address alignment"));
    } else {
        crate::logging::logln(
            "pmm",
            format_args!(
                "[pmm-test] OK alignment: {:#x}, {:#x}, {:#x}",
                addr0, addr1, addr2
            ),
        );

        print_test_line(format_args!("  [ OK ] physical address alignment"));
    }

    let reserved = |addr: u64| (KERNEL_OFFSET..STACK_TOP).contains(&addr);

    if reserved(addr0) || reserved(addr1) || reserved(addr2) {
        failures += 1;
        crate::logging::logln(
            "pmm",
            format_args!(
                "[pmm-test] FAIL reserved range hit: {:#x}, {:#x}, {:#x}",
                addr0, addr1, addr2
            ),
        );

        print_test_line(format_args!("  [FAIL] frame allocated in reserved range"));
    } else {
        crate::logging::logln("pmm", format_args!("[pmm-test] OK reserved range check"));
        print_test_line(format_args!("  [ OK ] reserved range is not allocated"));
    }

    let old_mid_pfn = frame1.pfn;
    let _ = release_test_pfn(frame1.pfn);
    let reused = match alloc_test_frame() {
        Some(f) => f,
        None => {
            crate::logging::logln(
                "pmm",
                format_args!("[pmm-test] FAIL re-allocation after release"),
            );

            print_test_line(format_args!("  [FAIL] re-allocation after release"));
            let _ = release_test_pfn(frame2.pfn);
            let _ = release_test_pfn(frame0.pfn);

            return;
        }
    };

    if reused.pfn != old_mid_pfn {
        failures += 1;
        crate::logging::logln(
            "pmm",
            format_args!(
                "[pmm-test] FAIL reuse mismatch: expected {}, got {}",
                old_mid_pfn, reused.pfn
            ),
        );

        print_test_line(format_args!("  [FAIL] released frame was not reused first"));
    } else {
        crate::logging::logln(
            "pmm",
            format_args!("[pmm-test] OK frame reuse ({})", reused.pfn),
        );

        print_test_line(format_args!("  [ OK ] released frame is reused"));
    }

    let _ = release_test_pfn(reused.pfn);
    let _ = release_test_pfn(frame2.pfn);
    let _ = release_test_pfn(frame0.pfn);

    // Step 2: run stress loop with short PMM lock sections per iteration.
    for i in 0..stress_iters {
        let f = match alloc_test_frame() {
            Some(f) => f,
            None => {
                failures += 1;
                crate::logging::logln(
                    "pmm",
                    format_args!("[pmm-test] FAIL stress alloc at iter {}", i),
                );
                print_test_line(format_args!("  [FAIL] stress alloc failed at iter {}", i));

                break;
            }
        };

        if f.physical_address() % PAGE_SIZE != 0 {
            failures += 1;
            crate::logging::logln(
                "pmm",
                format_args!(
                    "[pmm-test] FAIL stress alignment at iter {} addr={:#x}",
                    i,
                    f.physical_address()
                ),
            );

            print_test_line(format_args!("  [FAIL] stress alignment at iter {}", i));
            let _ = release_test_pfn(f.pfn);

            break;
        }

        let _ = release_test_pfn(f.pfn);

        if i != 0 && i % 512 == 0 {
            crate::logging::logln(
                "pmm",
                format_args!("[pmm-test] stress progress: {}/{}", i, stress_iters),
            );
        }
    }

    if failures == 0 {
        crate::logging::logln(
            "pmm",
            format_args!("[pmm-test] OK stress {} cycles", stress_iters),
        );

        print_test_line(format_args!(
            "  [ OK ] stress {} alloc/release cycles",
            stress_iters
        ));
    }

    if failures == 0 {
        crate::logging::logln("pmm", format_args!("[pmm-test] PASSED"));
        print_test_line(format_args!("PMM self-test PASSED"));
    } else {
        crate::logging::logln(
            "pmm",
            format_args!("[pmm-test] FAILED ({} issue(s))", failures),
        );

        print_test_line(format_args!("PMM self-test FAILED ({} issue(s))", failures));
    }
}
