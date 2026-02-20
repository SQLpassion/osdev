//! Kernel heap manager.
//!
//! Design summary:
//! - Contiguous heap region with variable-sized blocks.
//! - Segregated free-list strategy with intrusive free nodes in free blocks.
//! - One header per block (`HeapBlockHeader`) storing `size`, `in_use` flag,
//!   and `prev_size` for O(1) neighbor lookup.
//! - Block splitting on allocation and O(1) adjacent coalescing on free.
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
/// Size of intrusive node stored in payload of free blocks.
const FREE_NODE_SIZE: usize = size_of::<FreeListNode>();
/// Global heap payload alignment.
const ALIGNMENT: usize = align_of::<usize>();

/// Returns `value` rounded up to `align` (power-of-two).
const fn align_up_const(value: usize, align: usize) -> usize {
    (value + (align - 1)) & !(align - 1)
}

/// Minimum free block size that can hold header + intrusive node.
const MIN_FREE_BLOCK_SIZE: usize = align_up_const(HEADER_SIZE + FREE_NODE_SIZE, ALIGNMENT);

/// Minimum tail size that is still worth splitting into a new free block.
const MIN_SPLIT_SIZE: usize = MIN_FREE_BLOCK_SIZE;

/// Number of segregated free-list bins.
const FREE_BIN_COUNT: usize = 32;

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

    /// Full size of the physically previous block.
    prev_size: usize,
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

    /// Returns the size of the physically previous block.
    #[inline]
    fn prev_size(&self) -> usize {
        self.prev_size
    }

    /// Stores the size of the physically previous block.
    #[inline]
    fn set_prev_size(&mut self, size: usize) {
        self.prev_size = size;
    }
}

/// Intrusive links stored in payload of free blocks.
#[repr(C)]
struct FreeListNode {
    prev: usize,
    next: usize,
}

/// Mutable heap bounds guarded by the global spinlock.
struct HeapState {
    /// Start address of the managed heap region.
    heap_start: usize,

    /// End address (exclusive) of the managed heap region.
    heap_end: usize,

    /// Segregated free-list heads, grouped by block size class.
    free_bins: [Option<usize>; FREE_BIN_COUNT],

    /// Bit-set for non-empty bins to accelerate candidate lookup.
    free_bin_bitmap: u64,
}

impl HeapState {
    const fn new() -> Self {
        Self {
            heap_start: 0,
            heap_end: 0,
            free_bins: [None; FREE_BIN_COUNT],
            free_bin_bitmap: 0,
        }
    }

    fn reset_free_bins(&mut self) {
        self.free_bins = [None; FREE_BIN_COUNT];
        self.free_bin_bitmap = 0;
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

/// Converts a free-block header pointer to its intrusive free-list node.
#[inline]
fn free_node_ptr(block: *mut HeapBlockHeader) -> *mut FreeListNode {
    payload_ptr(block).cast::<FreeListNode>()
}

/// Converts a nullable raw block pointer to an address (`0` means null).
#[inline]
fn ptr_to_addr(block: *mut HeapBlockHeader) -> usize {
    block as usize
}

/// Converts an address to a raw block pointer (`0` maps to null).
#[inline]
fn addr_to_ptr(addr: usize) -> *mut HeapBlockHeader {
    addr as *mut HeapBlockHeader
}

/// Returns the index of the bin responsible for `block_size`.
#[inline]
fn size_class_index(block_size: usize) -> usize {
    let normalized = block_size.max(MIN_FREE_BLOCK_SIZE);
    let log2 = (usize::BITS as usize - 1).saturating_sub(normalized.leading_zeros() as usize);
    let base =
        (usize::BITS as usize - 1).saturating_sub(MIN_FREE_BLOCK_SIZE.leading_zeros() as usize);
    let raw = log2.saturating_sub(base);
    raw.min(FREE_BIN_COUNT - 1)
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

    with_heap(|state| {
        // Step 1: Reset heap bounds and free-list metadata for a clean init state.
        state.heap_start = heap_start;
        state.heap_end = heap_end;
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
        }

        insert_free_block(state, header_at(heap_start));
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
            // Step 1: Find a candidate free block using segregated bins.
            if let Some(block) = find_suitable_free_block(state, size) {
                let ptr = allocate_block(state, block, size);
                return AllocAttempt::Allocated(ptr);
            }

            // Step 2: Grow heap when no fitting block currently exists.
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
        FreeResult::Freed {
            block_size: final_size,
        }
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

/// Inserts a free block into its segregated bin as the list head.
fn insert_free_block(state: &mut HeapState, block: *mut HeapBlockHeader) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `block` points to a valid free block header while heap lock is held.
    unsafe {
        let header = &mut *block;
        let size = header.size();
        if size < MIN_FREE_BLOCK_SIZE {
            return;
        }

        let idx = size_class_index(size);
        let head_addr = state.free_bins[idx].unwrap_or(0);
        let head = addr_to_ptr(head_addr);
        let node = &mut *free_node_ptr(block);

        // Step 1: Link the new node before the current head.
        node.prev = 0;
        node.next = head_addr;

        // Step 2: Repair previous-pointer of old head when list was non-empty.
        if !head.is_null() {
            let head_node = &mut *free_node_ptr(head);
            head_node.prev = ptr_to_addr(block);
        }

        // Step 3: Publish new head and set the non-empty bitmap bit.
        state.free_bins[idx] = Some(ptr_to_addr(block));
        state.free_bin_bitmap |= 1u64 << idx;
    }
}

/// Removes a free block from its segregated bin.
fn remove_free_block(state: &mut HeapState, block: *mut HeapBlockHeader) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `block` points to a free block currently linked in its bin while heap lock is held.
    unsafe {
        let header = &*block;
        let size = header.size();
        if size < MIN_FREE_BLOCK_SIZE {
            return;
        }

        let idx = size_class_index(size);
        let node = &mut *free_node_ptr(block);
        let prev_addr = node.prev;
        let next_addr = node.next;
        let prev = addr_to_ptr(prev_addr);
        let next = addr_to_ptr(next_addr);

        // Step 1: Relink predecessor or update bin head when removing first element.
        if prev.is_null() {
            state.free_bins[idx] = if next.is_null() {
                None
            } else {
                Some(next_addr)
            };
        } else {
            let prev_node = &mut *free_node_ptr(prev);
            prev_node.next = next_addr;
        }

        // Step 2: Relink successor back to predecessor.
        if !next.is_null() {
            let next_node = &mut *free_node_ptr(next);
            next_node.prev = prev_addr;
        }

        // Step 3: Clear local links and update bitmap when list becomes empty.
        node.prev = 0;
        node.next = 0;

        if state.free_bins[idx].is_none() {
            state.free_bin_bitmap &= !(1u64 << idx);
        }
    }
}

/// Finds and unlinks a free block with `size >= requested_size`.
fn find_suitable_free_block(
    state: &mut HeapState,
    requested_size: usize,
) -> Option<*mut HeapBlockHeader> {
    let start_idx = size_class_index(requested_size);
    let mut remaining = state.free_bin_bitmap & (!0u64 << start_idx);

    // Step 1: Visit bins from small-to-large size classes via bitmap iteration.
    while remaining != 0 {
        let idx = remaining.trailing_zeros() as usize;
        remaining &= !(1u64 << idx);

        let mut current = addr_to_ptr(state.free_bins[idx].unwrap_or(0));

        // Step 2: Scan a bin until a fitting block is found.
        while !current.is_null() {
            // SAFETY:
            // - This requires `unsafe` because it dereferences raw pointers.
            // - `current` is a node in a free-list bin while heap lock is held.
            let (block_size, next_addr) = unsafe {
                let header = &*current;
                let node = &*free_node_ptr(current);
                (header.size(), node.next)
            };

            if block_size >= requested_size {
                remove_free_block(state, current);
                return Some(current);
            }

            current = addr_to_ptr(next_addr);
        }
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

/// Updates `prev_size` of the physical successor block, if present.
fn update_next_prev_size(state: &mut HeapState, block_addr: usize, block_size: usize) {
    let Some(next_addr) = block_addr.checked_add(block_size) else {
        return;
    };
    if next_addr >= state.heap_end {
        return;
    }

    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `next_addr` is inside the heap and points to the immediate successor header.
    unsafe {
        let next_header = &mut *header_at(next_addr);
        next_header.set_prev_size(block_size);
    }
}

/// Splits and marks a selected free block as allocated.
fn allocate_block(state: &mut HeapState, block: *mut HeapBlockHeader, size: usize) -> *mut u8 {
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `block` points to a valid free block removed from bins under heap lock.
    unsafe {
        let header = &mut *block;
        let old_size = header.size();

        // Step 1: Split when the remainder can host a valid free block.
        if old_size >= size + MIN_SPLIT_SIZE {
            header.set_in_use(true);
            header.set_size(size);

            let tail_addr = (block as usize).saturating_add(size);
            let tail_header = &mut *header_at(tail_addr);
            tail_header.set_in_use(false);
            tail_header.set_size(old_size - size);
            tail_header.set_prev_size(size);

            // Step 2: Repair successor boundary-tag and insert split remainder.
            update_next_prev_size(state, tail_addr, tail_header.size());
            insert_free_block(state, header_at(tail_addr));
        } else {
            // Step 3: Consume full block when split remainder would be too small.
            header.set_in_use(true);
            update_next_prev_size(state, block as usize, old_size);
        }
    }

    payload_ptr(block)
}

/// Coalesces `block` with physically adjacent free neighbors and returns the merged block.
fn coalesce_free_block(state: &mut HeapState, block: *mut HeapBlockHeader) -> *mut HeapBlockHeader {
    let mut coalesced = block;

    // Step 1: Merge with previous free neighbor using `prev_size` boundary-tag.
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `coalesced` points to a valid heap block and heap lock is held.
    unsafe {
        let header = &*coalesced;
        let prev_size = header.prev_size();
        if prev_size >= HEADER_SIZE {
            let prev_addr = (coalesced as usize).saturating_sub(prev_size);
            if prev_addr >= state.heap_start {
                let prev = header_at(prev_addr);
                let prev_header = &*prev;
                if !prev_header.in_use() {
                    remove_free_block(state, prev);

                    let new_size = prev_header.size().saturating_add(header.size());
                    let prev_mut = &mut *prev;
                    prev_mut.set_size(new_size);
                    prev_mut.set_in_use(false);

                    coalesced = prev;
                }
            }
        }
    }

    // Step 2: Merge with following free neighbor when present.
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `coalesced` remains a valid heap block under lock.
    unsafe {
        let header = &*coalesced;
        let next_addr = (coalesced as usize).saturating_add(header.size());
        if next_addr < state.heap_end {
            let next = header_at(next_addr);
            let next_header = &*next;
            if !next_header.in_use() && next_header.size() >= HEADER_SIZE {
                remove_free_block(state, next);

                let merged_size = header.size().saturating_add(next_header.size());
                let merged = &mut *coalesced;
                merged.set_size(merged_size);
                merged.set_in_use(false);
            }
        }
    }

    // Step 3: Repair successor boundary-tag for the final merged block.
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `coalesced` is still a valid in-heap block.
    let final_size = unsafe { (&*coalesced).size() };
    update_next_prev_size(state, coalesced as usize, final_size);

    coalesced
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

    // Step 1: Find the previous tail block so the new block gets correct `prev_size`.
    let prev_block_size = if old_end == state.heap_start {
        0
    } else {
        let mut current = state.heap_start;
        let mut tail_size = 0;
        while current < old_end {
            // SAFETY:
            // - This requires `unsafe` because it dereferences raw pointers.
            // - `current` iterates through validated heap block headers.
            let header = unsafe { &*header_at(current) };
            let size = header.size();
            if size < HEADER_SIZE {
                return false;
            }

            let Some(next) = current.checked_add(size) else {
                return false;
            };
            if next > old_end {
                return false;
            }

            tail_size = size;
            current = next;
        }

        if current != old_end {
            return false;
        }

        tail_size
    };

    // Step 2: Materialize a new free block in the freshly extended range.
    // SAFETY:
    // - This requires `unsafe` because it dereferences raw pointers.
    // - `old_end..new_end` is the newly appended heap region.
    unsafe {
        let header = &mut *header_at(old_end);
        header.set_in_use(false);
        header.set_size(amount);
        header.set_prev_size(prev_block_size);
    }

    state.heap_end = new_end;

    // Step 3: Coalesce with neighboring free tail and enqueue once.
    let block = coalesce_free_block(state, header_at(old_end));
    insert_free_block(state, block);
    true
}

#[inline]
fn compute_heap_growth_for_request(required_block_size: usize) -> usize {
    align_up_checked(required_block_size, HEAP_GROWTH).unwrap_or(HEAP_GROWTH)
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
