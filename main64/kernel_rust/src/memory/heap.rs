//! Kernel heap manager.
//!
//! Design summary:
//! - Contiguous heap region with variable-sized blocks.
//! - First-fit allocation strategy.
//! - One header per block (`HeapBlockHeader`) storing `size` + `in_use` flag.
//! - Block splitting on allocation and block coalescing on free.
//! - Backed by a global spinlock for synchronized access.
//!
//! Notes:
//! - Block size includes the header itself.
//! - Payload pointer is always `header + HEADER_SIZE`.
//! - Heap growth is page-sized (`HEAP_GROWTH`) and relies on demand paging.

use alloc::vec::Vec;
use core::fmt::Write;
use core::mem::{align_of, size_of};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::screen::Screen;
use crate::logging;
use crate::sync::spinlock::SpinLock;

/// Size of one block header in bytes.
const HEADER_SIZE: usize = size_of::<HeapBlockHeader>();
/// Global heap payload alignment.
const ALIGNMENT: usize = align_of::<usize>();
/// Minimum tail size that is still worth splitting into a new block.
const MIN_SPLIT_SIZE: usize = HEADER_SIZE + 1;

/// Virtual start address of the kernel heap arena.
const HEAP_START_OFFSET: usize = 0xFFFF_8000_0050_0000;
/// Heap size after `init()`.
const INITIAL_HEAP_SIZE: usize = 0x1000;
/// Increment used when extending the heap arena.
const HEAP_GROWTH: usize = 0x1000;
/// Hard upper bound for total managed heap bytes.
const MAX_HEAP_SIZE: usize = 0x0100_0000; // 16 MiB

/// LSB encodes allocation state in `size_and_flags`.
const IN_USE_MASK: usize = 0x1;
/// Remaining bits encode block size.
const SIZE_MASK: usize = !IN_USE_MASK;

/// Per-block metadata stored directly in heap memory.
#[repr(C)]
struct HeapBlockHeader {
    /// Packed representation: `[size bits | in-use bit]`.
    size_and_flags: usize,
}

impl HeapBlockHeader {
    /// Returns full block size in bytes (header + payload).
    #[inline]
    fn size(&self) -> usize {
        self.size_and_flags & SIZE_MASK
    }

    /// Updates size bits while preserving the in-use flag.
    #[inline]
    fn set_size(&mut self, size: usize) {
        let flags = self.size_and_flags & IN_USE_MASK;
        self.size_and_flags = flags | (size & SIZE_MASK);
    }

    /// Returns whether this block is currently allocated.
    #[inline]
    fn in_use(&self) -> bool {
        (self.size_and_flags & IN_USE_MASK) != 0
    }

    /// Sets or clears the in-use bit.
    #[inline]
    fn set_in_use(&mut self, in_use: bool) {
        if in_use {
            self.size_and_flags |= IN_USE_MASK;
        } else {
            self.size_and_flags &= SIZE_MASK;
        }
    }
}

/// Mutable heap bounds guarded by the global spinlock.
struct HeapState {
    /// Start address of the managed heap region.
    heap_start: usize,
    /// End address (exclusive) of the managed heap region.
    heap_end: usize,
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
            inner: SpinLock::new(HeapState {
                heap_start: 0,
                heap_end: 0,
            }),
            initialized: AtomicBool::new(false),
            serial_debug_enabled: AtomicBool::new(false),
        }
    }
}

/// Process-wide heap instance.
static HEAP: GlobalHeap = GlobalHeap::new();

/// Aligns `value` up to the next `align` boundary.
#[inline]
fn align_up_checked(value: usize, align: usize) -> Option<usize> {
    let mask = align.checked_sub(1)?;
    value.checked_add(mask).map(|v| v & !mask)
}

/// Computes the full aligned block size for a payload request.
#[inline]
fn compute_aligned_heapblock_size(requested_size: usize) -> Option<usize> {
    requested_size
        .checked_add(HEADER_SIZE)
        .and_then(|v| align_up_checked(v, ALIGNMENT))
}

/// Reinterprets an address as a mutable block-header pointer.
#[inline]
fn header_at(addr: usize) -> *mut HeapBlockHeader {
    addr as *mut HeapBlockHeader
}

/// Converts a block header pointer to the corresponding payload pointer.
#[inline]
fn payload_ptr(block: *mut HeapBlockHeader) -> *mut u8 {
    block.cast::<u8>().wrapping_add(HEADER_SIZE)
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

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `heap_start..heap_end` is the reserved kernel heap region.
    // - The VMM will demand-map pages on access.
    // - We only zero the initial heap range.
    unsafe {
        core::ptr::write_bytes(heap_start as *mut u8, 0, INITIAL_HEAP_SIZE);
    }

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `heap_start` is aligned and points to the start of the heap.
    // - The heap range is writable.
    unsafe {
        let header = &mut *header_at(heap_start);
        header.set_in_use(false);
        header.set_size(INITIAL_HEAP_SIZE);
    }

    with_heap(|state| {
        state.heap_start = heap_start;
        state.heap_end = heap_end;
    });

    HEAP.serial_debug_enabled
        .store(debug_output, Ordering::Release);
    HEAP.initialized.store(true, Ordering::Release);
    INITIAL_HEAP_SIZE
}

#[inline]
fn serial_debug_enabled() -> bool {
    HEAP.serial_debug_enabled.load(Ordering::Acquire)
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

/// Public alignment constant for components that prefer const access.
pub const HEAP_ALIGNMENT: usize = ALIGNMENT;

/// Allocates `size` bytes and returns a pointer to the payload.
pub fn malloc(size: usize) -> *mut u8 {
    let requested_size = size;
    let Some(size) = compute_aligned_heapblock_size(requested_size) else {
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP] alloc failed (overflow) requested={}",
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
            if let Some(block) = find_block_in_heap(state, size) {
                allocate_block(block, size);
                return AllocAttempt::Allocated(payload_ptr(block));
            }

            let growth = compute_heap_growth_for_request(size);
            if grow_heap(state, growth) {
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
                        "[HEAP] alloc ptr={:#x} requested={} block={}",
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
                        "[HEAP] alloc failed (grow) requested={} block={}",
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
        let Some(block) = find_block_by_payload_ptr(state, ptr) else {
            return FreeResult::Rejected {
                reason: "invalid pointer",
            };
        };

        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `block` was found via a full heap walk matching `payload_ptr(block) == ptr`.
        // - Therefore it points to a valid block header.
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

        header.set_in_use(false);
        let _ = merge_free_blocks(state);
        FreeResult::Freed { block_size }
    });

    match result {
        FreeResult::Freed { block_size } => {
            logging::logln_with_options(
                "heap",
                format_args!("[HEAP] free ptr={:#x} block={}", ptr as usize, block_size),
                serial_debug_enabled(),
                true,
            );
        }
        FreeResult::Rejected { reason } => {
            logging::logln_with_options(
                "heap",
                format_args!(
                    "[HEAP] free rejected ptr={:#x} reason={}",
                    ptr as usize, reason
                ),
                serial_debug_enabled(),
                true,
            );
        }
    }
}

fn find_block_in_heap(state: &HeapState, size: usize) -> Option<*mut HeapBlockHeader> {
    let mut current = state.heap_start;
    while current < state.heap_end {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `current` is within the heap bounds.
        // - The heap region is mapped on demand.
        let header = unsafe { &*header_at(current) };
        let block_size = header.size();

        if block_size < HEADER_SIZE {
            break;
        }

        // First-fit strategy: return the first sufficiently large free block.
        if !header.in_use() && block_size >= size {
            return Some(header_at(current));
        }

        current = current.saturating_add(block_size);
    }
    None
}

fn find_block_by_payload_ptr(state: &HeapState, ptr: *mut u8) -> Option<*mut HeapBlockHeader> {
    let ptr_addr = ptr as usize;
    let min_payload = state.heap_start.checked_add(HEADER_SIZE)?;
    if ptr_addr < min_payload || ptr_addr >= state.heap_end {
        return None;
    }

    let mut current = state.heap_start;
    while current < state.heap_end {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `current` is within heap bounds and points to a block header.
        let header = unsafe { &*header_at(current) };
        let block_size = header.size();
        if block_size < HEADER_SIZE {
            return None;
        }

        let next_addr = current.checked_add(block_size)?;
        if next_addr > state.heap_end {
            return None;
        }

        let block = header_at(current);
        if payload_ptr(block) as usize == ptr_addr {
            return Some(block);
        }
        current = next_addr;
    }
    None
}

fn allocate_block(block: *mut HeapBlockHeader, size: usize) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `block` points to a valid heap block header.
    // - `size` is aligned and includes the header.
    unsafe {
        let header = &mut *block;
        let old_size = header.size();
        if old_size >= size + MIN_SPLIT_SIZE {
            // Split block into: allocated head + free tail.
            header.set_in_use(true);
            header.set_size(size);

            let next_addr = (block as usize).saturating_add(size);
            let next_header = &mut *header_at(next_addr);
            next_header.set_in_use(false);
            next_header.set_size(old_size - size);
        } else {
            // Tail would be too small: consume entire block.
            header.set_in_use(true);
        }
    }
}

/// Merges adjacent free blocks and returns the number of merges performed.
///
/// Requires the global heap lock (`HEAP.inner`) to already be held by the caller
/// via `with_heap`, i.e. this function must only run with exclusive heap access.
fn merge_free_blocks(state: &mut HeapState) -> usize {
    let mut merged = 0;
    let mut current = state.heap_start;
    while current < state.heap_end {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `current` is within the heap.
        let header = unsafe { &mut *header_at(current) };
        let size = header.size();
        if size < HEADER_SIZE {
            break;
        }

        let Some(mut next_addr) = current.checked_add(size) else {
            break;
        };
        if next_addr >= state.heap_end {
            break;
        }

        if header.in_use() {
            current = next_addr;
            continue;
        }

        let mut merged_size = size;
        loop {
            if next_addr >= state.heap_end {
                break;
            }

            // SAFETY:
            // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
            // - `next_addr` is within the heap.
            let next_header = unsafe { &*header_at(next_addr) };
            let next_size = next_header.size();
            if next_size < HEADER_SIZE || next_header.in_use() {
                break;
            }

            let Some(new_size) = merged_size.checked_add(next_size) else {
                break;
            };
            merged_size = new_size;
            merged += 1;
            let Some(after_next) = next_addr.checked_add(next_size) else {
                break;
            };
            if after_next > state.heap_end {
                break;
            }
            next_addr = after_next;
        }

        if merged_size != size {
            header.set_size(merged_size);
            let Some(next_current) = current.checked_add(merged_size) else {
                break;
            };
            current = next_current;
        } else {
            current = next_addr;
        }
    }
    merged
}

/// Appends a new free block at the current heap end and coalesces neighbors.
///
/// Requires the global heap lock (`HEAP.inner`) to already be held by the caller
/// via `with_heap`, i.e. this function must only run with exclusive heap access.
fn grow_heap(state: &mut HeapState, amount: usize) -> bool {
    if amount < HEADER_SIZE {
        return false;
    }

    let current_size = state.heap_end.saturating_sub(state.heap_start);
    if current_size >= MAX_HEAP_SIZE {
        return false;
    }

    let remaining = MAX_HEAP_SIZE - current_size;
    if amount > remaining {
        return false;
    }

    let old_end = state.heap_end;
    let Some(new_end) = old_end.checked_add(amount) else {
        return false;
    };

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `old_end` is the previous heap end, so it is safe to place a new block there.
    // - Pages are mapped on demand via the VMM.
    unsafe {
        let header = &mut *header_at(old_end);
        header.set_in_use(false);
        header.set_size(amount);
    }

    state.heap_end = new_end;
    let _ = merge_free_blocks(state);
    true
}

#[inline]
fn compute_heap_growth_for_request(required_block_size: usize) -> usize {
    align_up_checked(required_block_size, HEAP_GROWTH).unwrap_or(HEAP_GROWTH)
}

/// Returns `(block_size, in_use)` for a block at `base + offset`.
///
/// This helper is intended for heap self-tests to validate internal layout.
fn read_block_metadata(base: usize, offset: usize) -> (usize, bool) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Caller ensures `base + offset` points into the heap.
    unsafe {
        let header = &*header_at(base + offset);
        (header.size(), header.in_use())
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
    if is_initialized() {
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] reinitializing heap"),
            serial_debug_enabled(),
            true,
        );
    }
    init(serial_debug_enabled());

    let heap_base = HEAP_START_OFFSET;
    let ptr1 = malloc(100);
    let ptr2 = malloc(100);

    let (size1, in_use1) = read_block_metadata(heap_base, 0);
    let (size2, in_use2) = read_block_metadata(heap_base, 112);
    let (size3, in_use3) = read_block_metadata(heap_base, 224);

    if size1 == 112 && in_use1 && size2 == 112 && in_use2 && size3 == 3872 && !in_use3 {
        writeln!(screen, "  [ OK ] initial allocation layout").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] OK initial layout"),
            serial_debug_enabled(),
            true,
        );
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] initial allocation layout").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP-TEST] FAIL initial layout: ({},{}), ({},{}), ({},{})",
                size1, in_use1, size2, in_use2, size3, in_use3
            ),
            serial_debug_enabled(),
            true,
        );
    }

    free(ptr1);
    let (size1, in_use1) = read_block_metadata(heap_base, 0);
    let (size2, in_use2) = read_block_metadata(heap_base, 112);
    if size1 == 112 && !in_use1 && size2 == 112 && in_use2 {
        writeln!(screen, "  [ OK ] free first block").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] OK free first block"),
            serial_debug_enabled(),
            true,
        );
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] free first block").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP-TEST] FAIL free first block: ({},{}), ({},{})",
                size1, in_use1, size2, in_use2
            ),
            serial_debug_enabled(),
            true,
        );
    }

    let ptr3 = malloc(50);
    let (size1, in_use1) = read_block_metadata(heap_base, 0);
    let (size2, in_use2) = read_block_metadata(heap_base, 64);
    if size1 == 64 && in_use1 && size2 == 48 && !in_use2 {
        writeln!(screen, "  [ OK ] split after 50-byte alloc").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] OK split after 50-byte alloc"),
            serial_debug_enabled(),
            true,
        );
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] split after 50-byte alloc").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP-TEST] FAIL split after 50-byte alloc: ({},{}), ({},{})",
                size1, in_use1, size2, in_use2
            ),
            serial_debug_enabled(),
            true,
        );
    }

    let ptr4 = malloc(40);
    let (size2, in_use2) = read_block_metadata(heap_base, 64);
    if size2 == 48 && in_use2 {
        writeln!(screen, "  [ OK ] allocate 40-byte block").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] OK allocate 40-byte block"),
            serial_debug_enabled(),
            true,
        );
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] allocate 40-byte block").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP-TEST] FAIL allocate 40-byte block: ({},{})",
                size2, in_use2
            ),
            serial_debug_enabled(),
            true,
        );
    }

    free(ptr2);
    free(ptr3);
    free(ptr4);

    let (size1, in_use1) = read_block_metadata(heap_base, 0);
    if size1 == INITIAL_HEAP_SIZE && !in_use1 {
        writeln!(screen, "  [ OK ] merge after frees").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!("[HEAP-TEST] OK merge after frees"),
            serial_debug_enabled(),
            true,
        );
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] merge after frees").unwrap();
        logging::logln_with_options(
            "heap",
            format_args!(
                "[HEAP-TEST] FAIL merge after frees: ({},{})",
                size1, in_use1
            ),
            serial_debug_enabled(),
            true,
        );
    }

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
