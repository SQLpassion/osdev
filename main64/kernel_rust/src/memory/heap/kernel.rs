//! Submodule containing the Ring 0 bare-metal kernel allocator implementation.
//!
//! Design summary:
//! - Implements [`KernelHeapEnv`] to satisfy the demand-paging heap growth model in Ring 0.
//! - Declares the global [`GlobalHeap`] wrapper around [`HeapState`] protected by a spinlock.
//! - Exposes standard public heap entry points: [`init`], [`malloc`], [`free`], and debug settings.
//! - Implements automated Ring 0 self-tests (`run_self_test`) to verify allocator health on boot.
//!
//! Concurrency:
//! - All global heap allocations and deallocations acquire the global spinlock [`HEAP.inner`] to ensure thread safety.

use crate::drivers::screen::Screen;
use crate::logging;
use crate::memory::pmm;
use crate::sync::spinlock::SpinLock;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

use super::generic::HeapEnvironment;
use super::types::{
    HeapState,
    compute_aligned_heapblock_size, find_suitable_free_block,
    allocate_block, compute_heap_growth_for_request, grow_heap,
    find_block_by_payload_ptr, coalesce_free_block, insert_free_block,
    header_at, HEADER_SIZE, INITIAL_HEAP_SIZE, HEAP_START_OFFSET,
    SYSTEM_HEAP_RESERVE_MIN_BYTES, HEAP_GROWTH, align_up_checked,
};

pub struct KernelHeapEnv;

impl HeapEnvironment for KernelHeapEnv {
    fn map_memory(&self, _start: usize, _end: usize) -> bool {
        // The kernel uses demand paging via the page fault handler,
        // so we don't need to do any explicit mapping during growth.
        true
    }

    fn max_heap_size(&self) -> usize {
        compute_system_heap_cap()
    }

    fn log(&self, msg: &str) {
        logging::logln_with_options(
            "heap",
            format_args!("[KERNEL HEAP] {}", msg),
            serial_debug_enabled(),
            true,
        );
    }
}

/// Global heap singleton.
struct GlobalHeap {
    /// Protected mutable heap state.
    inner: SpinLock<HeapState>,

    /// Set to `true` after `init()` completed.
    initialized: AtomicBool,

    /// Controls whether heap logs are emitted to serial output.
    serial_debug_enabled: AtomicBool,
}

impl GlobalHeap {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(HeapState::new()),
            initialized: AtomicBool::new(false),
            serial_debug_enabled: AtomicBool::new(false),
        }
    }
}

/// Process-wide heap instance.
static HEAP: GlobalHeap = GlobalHeap::new();

/// Computes the heap cap strictly from current PMM free capacity.
fn compute_system_heap_cap() -> usize {
    // Step 1: PMM may be unavailable in early bring-up contexts.
    // In that case, keep the heap at its initial single-page size.
    if !pmm::is_initialized() {
        return INITIAL_HEAP_SIZE;
    }

    // Step 2: Convert free frame count into bytes with checked arithmetic.
    let free_frames = pmm::free_frame_count();
    let free_bytes = free_frames.saturating_mul(pmm::PAGE_SIZE) as usize;

    // Step 3: Keep explicit PMM headroom for page tables, user mappings, and
    // non-heap PMM clients so heap growth cannot consume the entire frame pool.
    let reserve_bytes = SYSTEM_HEAP_RESERVE_MIN_BYTES.max(free_bytes / 8);
    let capped_bytes = free_bytes.saturating_sub(reserve_bytes);

    // Step 4: Enforce allocator invariants:
    // - cap never drops below initial arena size
    // - cap remains page-granular to match growth/page-fault semantics
    let with_floor = capped_bytes.max(INITIAL_HEAP_SIZE);
    align_up_checked(with_floor, HEAP_GROWTH).unwrap_or(with_floor)
}

/// Executes a closure with exclusive mutable access to heap state.
fn with_heap<R>(f: impl FnOnce(&mut HeapState) -> R) -> R {
    let mut guard = HEAP.inner.lock();
    f(&mut guard)
}

/// Initializes the heap manager and returns the heap size.
pub fn init(debug_output: bool) -> usize {
    let heap_start = HEAP_START_OFFSET;
    let heap_end = HEAP_START_OFFSET + INITIAL_HEAP_SIZE;
    let max_heap_size = compute_system_heap_cap();

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `heap_start..heap_end` is the reserved kernel heap region.
    // - The VMM will demand-map pages on access.
    // - We only zero the initial heap range.
    unsafe {
        core::ptr::write_bytes(heap_start as *mut u8, 0, INITIAL_HEAP_SIZE);
    }

    with_heap(|state| {
        // Step 1: Reset heap bounds and free-list metadata for a clean init state.
        state.heap_start = heap_start;
        state.heap_end = heap_end;
        state.max_heap_size = max_heap_size;
        state.reset_free_bins();

        // Step 2: Create a single initial free block spanning the full initial arena.
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `heap_start` is aligned and points to writable heap memory.
        unsafe {
            let header = &mut *header_at(heap_start);
            header.set_in_use(false);
            header.set_size(INITIAL_HEAP_SIZE);
            header.set_prev_size(0);
            header.set_magic_for_addr(heap_start);
        }

        insert_free_block(state, header_at(heap_start));

        // Step 3: The initial block is the only and therefore last physical block.
        state.tail_block_addr = heap_start;
    });

    HEAP.serial_debug_enabled
        .store(debug_output, Ordering::Release);
    HEAP.initialized.store(true, Ordering::Release);

    // Step 4: Emit the derived system cap once so OOM behavior is auditable.
    logging::logln_with_options(
        "heap",
        format_args!("[KERNEL HEAP] init cap={} bytes", max_heap_size),
        serial_debug_enabled(),
        true,
    );

    INITIAL_HEAP_SIZE
}

#[inline]
pub(crate) fn serial_debug_enabled() -> bool {
    HEAP.serial_debug_enabled.load(Ordering::Acquire)
}

/// Emits heap free-path diagnostics via the central logger.
#[inline]
fn log_free_diagnostic(args: core::fmt::Arguments<'_>) {
    // Step 1: Keep free-path logging on the non-allocating logger primitives only.
    // `logln_with_options` writes to serial and an in-memory fixed capture buffer,
    // so this diagnostic path does not require heap allocations.
    logging::logln_with_options("heap", args, serial_debug_enabled(), true);
}

/// Returns whether heap debug output to serial is enabled.
#[cfg_attr(not(test), allow(dead_code))]
pub fn debug_output_enabled() -> bool {
    serial_debug_enabled()
}

/// Enables or disables heap debug output and returns the previous setting.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_debug_output(enabled: bool) -> bool {
    HEAP.serial_debug_enabled.swap(enabled, Ordering::AcqRel)
}

/// Returns whether the heap manager has been initialized.
pub fn is_initialized() -> bool {
    HEAP.initialized.load(Ordering::Acquire)
}

/// Returns the current heap growth cap in bytes.
#[cfg_attr(not(test), allow(dead_code))]
pub fn max_heap_size() -> usize {
    with_heap(|state| state.max_heap_size)
}

/// Allocates `size` bytes and returns a pointer to the payload.
pub fn malloc(size: usize) -> *mut u8 {
    let requested_size = size;

    let Some(size) = compute_aligned_heapblock_size(requested_size) else {
        logging::logln_with_options(
            "heap",
            format_args!(
                "[KERNEL HEAP] alloc failed (overflow) requested={}",
                requested_size
            ),
            serial_debug_enabled(),
            true,
        );

        return core::ptr::null_mut();
    };

    enum AllocAttempt {
        Allocated(*mut u8),
        Retry,
        Fail,
    }

    loop {
        let attempt = with_heap(|state| {
            // Step 1: Find a candidate free block using segregated bins.
            if let Some(block) = find_suitable_free_block(state, size) {
                let ptr = allocate_block(state, block, size);
                return AllocAttempt::Allocated(ptr);
            }

            // Step 2: Grow heap when no fitting block currently exists.
            let growth = compute_heap_growth_for_request(size);

            if grow_heap(state, growth, &KernelHeapEnv) {
                AllocAttempt::Retry
            } else {
                AllocAttempt::Fail
            }
        });

        match attempt {
            AllocAttempt::Allocated(ptr) => {
                logging::logln_with_options(
                    "heap",
                    format_args!(
                        "[KERNEL HEAP] alloc ptr={:#x} requested={} block={}",
                        ptr as usize, requested_size, size
                    ),
                    serial_debug_enabled(),
                    true,
                );

                return ptr;
            }
            AllocAttempt::Retry => {}
            AllocAttempt::Fail => {
                logging::logln_with_options(
                    "heap",
                    format_args!(
                        "[KERNEL HEAP] alloc failed (grow) requested={} block={}",
                        requested_size, size
                    ),
                    serial_debug_enabled(),
                    true,
                );

                return core::ptr::null_mut();
            }
        }
    }
}

/// Frees a previously allocated heap pointer.
pub fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }

    enum FreeResult {
        Freed { block_size: usize },
        Rejected { reason: &'static str },
    }

    let result = with_heap(|state| {
        // Step 1: Validate pointer by exact payload-address match inside heap walk.
        let Some(block) = find_block_by_payload_ptr(state, ptr) else {
            return FreeResult::Rejected {
                reason: "invalid pointer",
            };
        };

        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `block` was found by exact payload match and therefore points to a valid block header.
        let header = unsafe { &mut *block };

        if !header.in_use() {
            return FreeResult::Rejected {
                reason: "double free",
            };
        }

        let block_size = header.size();

        if block_size < HEADER_SIZE {
            return FreeResult::Rejected {
                reason: "corrupt block header",
            };
        }

        // Step 2: Mark block free, coalesce with free neighbors, and enqueue once.
        header.set_in_use(false);
        let coalesced = coalesce_free_block(state, block);
        insert_free_block(state, coalesced);

        // SAFETY:
        // - This requires `unsafe` because it dereferences raw pointers.
        // - `coalesced` points to a valid heap header after coalescing.
        let final_size = unsafe { (&*coalesced).size() };

        // Step 3: If the coalesced block ends at `heap_end`, it is now the last physical block.
        if coalesced as usize + final_size == state.heap_end {
            state.tail_block_addr = coalesced as usize;
        }

        FreeResult::Freed {
            block_size: final_size,
        }
    });

    match result {
        FreeResult::Freed { block_size } => {
            log_free_diagnostic(format_args!(
                "[KERNEL HEAP] free ptr={:#x} block={}",
                ptr as usize, block_size
            ));
        }
        FreeResult::Rejected { reason } => {
            log_free_diagnostic(format_args!(
                "[KERNEL HEAP] free rejected ptr={:#x} reason={}",
                ptr as usize, reason
            ));
        }
    }
}

/// Runs heap self-tests and prints results to the screen.
pub fn run_self_test(screen: &mut Screen) {
    let mut failures = 0u32;
    logging::logln_with_options(
        "heap",
        format_args!("[HEAP-TEST] start"),
        serial_debug_enabled(),
        true,
    );

    // Step 1: Ensure the allocator is initialized, but never reset it once live.
    // Reinitializing in a running kernel would invalidate already-allocated state.
    if !is_initialized() {
        init(serial_debug_enabled());
    }

    // Step 2: Validate independent allocations and payload integrity.
    let ptr1 = malloc(100);
    let ptr2 = malloc(200);

    if ptr1.is_null() || ptr2.is_null() || ptr1 == ptr2 {
        failures += 1;
        writeln!(screen, "  [FAIL] independent allocation layout").unwrap();
    } else {
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `ptr1` and `ptr2` are non-null pointers returned by `malloc`.
        // - Writes and reads are bounded to requested allocation sizes.
        unsafe {
            core::ptr::write_bytes(ptr1, 0xA1, 100);
            core::ptr::write_bytes(ptr2, 0xB2, 200);

            let first_a = core::ptr::read_volatile(ptr1);
            let last_a = core::ptr::read_volatile(ptr1.add(99));
            let first_b = core::ptr::read_volatile(ptr2);
            let last_b = core::ptr::read_volatile(ptr2.add(199));

            if first_a == 0xA1 && last_a == 0xA1 && first_b == 0xB2 && last_b == 0xB2 {
                writeln!(screen, "  [ OK ] independent allocation layout").unwrap();
            } else {
                failures += 1;
                writeln!(screen, "  [FAIL] independent allocation layout").unwrap();
            }
        }
    }

    // Step 3: Validate free/coalesce path by freeing and requesting a larger block.
    free(ptr1);
    free(ptr2);
    let ptr3 = malloc(256);

    if !ptr3.is_null() {
        writeln!(screen, "  [ OK ] free+reuse large allocation").unwrap();
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] free+reuse large allocation").unwrap();
    }

    free(ptr3);

    let mut values: Vec<u64> = Vec::with_capacity(16);

    for i in 0..16u64 {
        values.push(i);
    }

    if values.len() == 16 && values[0] == 0 && values[15] == 15 {
        writeln!(screen, "  [ OK ] rust alloc (Vec) on heap").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] OK rust alloc (Vec)"),
            serial_debug_enabled(),
            true,
        );
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] rust alloc (Vec) on heap").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP-TEST] FAIL rust alloc (Vec): len={}, first={}, last={}",
                values.len(),
                values.first().copied().unwrap_or(0),
                values.last().copied().unwrap_or(0)
            ),
            serial_debug_enabled(),
            true,
        );
    }

    if failures == 0 {
        writeln!(screen, "Heap self-test complete (OK).").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] done (ok)"),
            serial_debug_enabled(),
            true,
        );
    } else {
        writeln!(screen, "Heap self-test complete ({} failures).", failures).unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] done (failures={})", failures),
            serial_debug_enabled(),
            true,
        );
    }
}
