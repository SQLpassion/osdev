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
    /// Tracks whether a newline sync was already emitted for heap logs.
    serial_line_synced: AtomicBool,
}

impl GlobalHeap {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(HeapState {
                heap_start: 0,
                heap_end: 0,
            }),
            initialized: AtomicBool::new(false),
            serial_line_synced: AtomicBool::new(false),
        }
    }
}

/// SAFETY:
/// - `inner` access is synchronized through `SpinLock`.
/// - `AtomicBool` fields are thread-safe.
unsafe impl Sync for GlobalHeap {}

/// Process-wide heap instance.
static HEAP: GlobalHeap = GlobalHeap::new();

/// Aligns `value` up to the next `align` boundary.
#[inline]
fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
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

/// Computes the block header pointer from a payload pointer.
#[inline]
fn block_from_payload(ptr: *mut u8) -> *mut HeapBlockHeader {
    ptr.wrapping_sub(HEADER_SIZE).cast::<HeapBlockHeader>()
}

/// Executes a closure with exclusive mutable access to heap state.
fn with_heap<R>(f: impl FnOnce(&mut HeapState) -> R) -> R {
    let mut guard = HEAP.inner.lock();
    f(&mut guard)
}

/// Initializes the heap manager and returns the heap size.
pub fn init() -> usize {
    let heap_start = HEAP_START_OFFSET;
    let heap_end = HEAP_START_OFFSET + INITIAL_HEAP_SIZE;

    // SAFETY:
    // - `heap_start..heap_end` is the reserved kernel heap region.
    // - The VMM will demand-map pages on access.
    // - We only zero the initial heap range.
    unsafe {
        core::ptr::write_bytes(heap_start as *mut u8, 0, INITIAL_HEAP_SIZE);
    }

    // SAFETY:
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

    HEAP.serial_line_synced.store(false, Ordering::Release);
    HEAP.initialized.store(true, Ordering::Release);
    INITIAL_HEAP_SIZE
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
    let mut size = size + HEADER_SIZE;
    size = align_up(size, ALIGNMENT);

    if let Some(block) = find_block(size) {
        allocate_block(block, size);
        let ptr = payload_ptr(block);
        heap_logln(format_args!(
            "[heap] alloc ptr={:#x} requested={} block={}",
            ptr as usize, requested_size, size
        ));
        return ptr;
    }

    // No suitable free block found: extend heap and retry.
    grow_heap(HEAP_GROWTH);
    malloc(size - HEADER_SIZE)
}

/// Frees a previously allocated heap pointer.
pub fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }

    let freed_block_size;
    // SAFETY:
    // - The pointer was previously returned by `malloc`.
    // - Subtracting `HEADER_SIZE` yields the block header.
    unsafe {
        let header = &mut *block_from_payload(ptr);
        freed_block_size = header.size();
        header.set_in_use(false);
    }
    heap_logln(format_args!(
        "[heap] free ptr={:#x} block={}",
        ptr as usize, freed_block_size
    ));

    while merge_free_blocks() > 0 {}
}

fn find_block(size: usize) -> Option<*mut HeapBlockHeader> {
    with_heap(|state| {
        let mut current = state.heap_start;
        while current < state.heap_end {
            // SAFETY:
            // - `current` is within the heap bounds.
            // - The heap region is mapped on demand.
            let header = unsafe { &mut *header_at(current) };
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
    })
}

fn allocate_block(block: *mut HeapBlockHeader, size: usize) {
    // SAFETY:
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
fn merge_free_blocks() -> usize {
    with_heap(|state| {
        let mut merged = 0;
        let mut current = state.heap_start;
        while current < state.heap_end {
            // SAFETY:
            // - `current` is within the heap.
            let header = unsafe { &mut *header_at(current) };
            let size = header.size();
            if size < HEADER_SIZE {
                break;
            }

            let next_addr = current.saturating_add(size);
            if next_addr >= state.heap_end {
                break;
            }

            // SAFETY:
            // - `next_addr` is within the heap.
            let next_header = unsafe { &mut *header_at(next_addr) };
            if !header.in_use() && !next_header.in_use() {
                header.set_size(size + next_header.size());
                merged += 1;
            } else {
                current = next_addr;
            }
        }
        merged
    })
}

fn grow_heap(amount: usize) {
    with_heap(|state| {
        let old_end = state.heap_end;
        let new_end = old_end.saturating_add(amount);

        // SAFETY:
        // - `old_end` is the previous heap end, so it is safe to place a new block there.
        // - Pages are mapped on demand via the VMM.
        unsafe {
            let header = &mut *header_at(old_end);
            header.set_in_use(false);
            header.set_size(amount);
        }

        state.heap_end = new_end;
    });

    // Merge with a possible free predecessor at the old heap end.
    let _ = merge_free_blocks();
}

#[inline]
fn heap_logln(args: core::fmt::Arguments<'_>) {
    // Ensure the first heap log after init starts on a fresh line in test output.
    if !HEAP.serial_line_synced.swap(true, Ordering::AcqRel) {
        logging::logln_with_options("heap", format_args!(""), true, false);
    }
    logging::logln("heap", args);
}

/// Returns `(block_size, in_use)` for a block at `base + offset`.
///
/// This helper is intended for heap self-tests to validate internal layout.
fn read_heapblock_metadata(base: usize, offset: usize) -> (usize, bool) {
    // SAFETY:
    // - Caller ensures `base + offset` points into the heap.
    unsafe {
        let header = &*header_at(base + offset);
        (header.size(), header.in_use())
    }
}

/// Runs heap self-tests and prints results to the screen.
pub fn run_self_test(screen: &mut Screen) {
    let mut failures = 0u32;
    heap_logln(format_args!("[heap-test] start"));
    if is_initialized() {
        heap_logln(format_args!("[heap-test] reinitializing heap"));
    }
    init();

    let heap_base = HEAP_START_OFFSET;
    let ptr1 = malloc(100);
    let ptr2 = malloc(100);

    let (size1, in_use1) = read_heapblock_metadata(heap_base, 0);
    let (size2, in_use2) = read_heapblock_metadata(heap_base, 112);
    let (size3, in_use3) = read_heapblock_metadata(heap_base, 224);

    if size1 == 112 && in_use1 && size2 == 112 && in_use2 && size3 == 3872 && !in_use3 {
        writeln!(screen, "  [ OK ] initial allocation layout").unwrap();
        heap_logln(format_args!("[heap-test] OK initial layout"));
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] initial allocation layout").unwrap();
        heap_logln(format_args!(
            "[heap-test] FAIL initial layout: ({},{}), ({},{}), ({},{})",
            size1, in_use1, size2, in_use2, size3, in_use3
        ));
    }

    free(ptr1);
    let (size1, in_use1) = read_heapblock_metadata(heap_base, 0);
    let (size2, in_use2) = read_heapblock_metadata(heap_base, 112);
    if size1 == 112 && !in_use1 && size2 == 112 && in_use2 {
        writeln!(screen, "  [ OK ] free first block").unwrap();
        heap_logln(format_args!("[heap-test] OK free first block"));
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] free first block").unwrap();
        heap_logln(format_args!(
            "[heap-test] FAIL free first block: ({},{}), ({},{})",
            size1, in_use1, size2, in_use2
        ));
    }

    let ptr3 = malloc(50);
    let (size1, in_use1) = read_heapblock_metadata(heap_base, 0);
    let (size2, in_use2) = read_heapblock_metadata(heap_base, 64);
    if size1 == 64 && in_use1 && size2 == 48 && !in_use2 {
        writeln!(screen, "  [ OK ] split after 50-byte alloc").unwrap();
        heap_logln(format_args!("[heap-test] OK split after 50-byte alloc"));
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] split after 50-byte alloc").unwrap();
        heap_logln(format_args!(
            "[heap-test] FAIL split after 50-byte alloc: ({},{}), ({},{})",
            size1, in_use1, size2, in_use2
        ));
    }

    let ptr4 = malloc(40);
    let (size2, in_use2) = read_heapblock_metadata(heap_base, 64);
    if size2 == 48 && in_use2 {
        writeln!(screen, "  [ OK ] allocate 40-byte block").unwrap();
        heap_logln(format_args!("[heap-test] OK allocate 40-byte block"));
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] allocate 40-byte block").unwrap();
        heap_logln(format_args!(
            "[heap-test] FAIL allocate 40-byte block: ({},{})",
            size2, in_use2
        ));
    }

    free(ptr2);
    free(ptr3);
    free(ptr4);

    let (size1, in_use1) = read_heapblock_metadata(heap_base, 0);
    if size1 == INITIAL_HEAP_SIZE && !in_use1 {
        writeln!(screen, "  [ OK ] merge after frees").unwrap();
        heap_logln(format_args!("[heap-test] OK merge after frees"));
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] merge after frees").unwrap();
        heap_logln(format_args!(
            "[heap-test] FAIL merge after frees: ({},{})",
            size1, in_use1
        ));
    }

    let mut values: Vec<u64> = Vec::with_capacity(16);
    for i in 0..16u64 {
        values.push(i);
    }
    if values.len() == 16 && values[0] == 0 && values[15] == 15 {
        writeln!(screen, "  [ OK ] rust alloc (Vec) on heap").unwrap();
        heap_logln(format_args!("[heap-test] OK rust alloc (Vec)"));
    } else {
        failures += 1;
        writeln!(screen, "  [FAIL] rust alloc (Vec) on heap").unwrap();
        heap_logln(format_args!(
            "[heap-test] FAIL rust alloc (Vec): len={}, first={}, last={}",
            values.len(),
            values.first().copied().unwrap_or(0),
            values.last().copied().unwrap_or(0)
        ));
    }

    if failures == 0 {
        writeln!(screen, "Heap self-test complete (OK).").unwrap();
        heap_logln(format_args!("[heap-test] done (ok)"));
    } else {
        writeln!(screen, "Heap self-test complete ({} failures).", failures).unwrap();
        heap_logln(format_args!("[heap-test] done (failures={})", failures));
    }
}
