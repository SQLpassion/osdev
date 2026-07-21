//! Submodule containing core types, constants, and basic list algorithms for the heap.
//!
//! Design summary:
//! - Defines metadata layouts: [`HeapBlockHeader`] and [`FreeListNode`].
//! - Defines [`HeapState`] which houses the segregated bin heads and bitmap.
//! - Implements key helper functions for size classing and address-to-pointer bridges.
//! - Implements shared core operational logic (insertion/removal in bins, search, coalescing, splitting).
//!
//! Safety:
//! - The algorithms here operate directly on raw memory addresses and pointers under the
//!   assumption that the caller has acquired appropriate exclusive locks (e.g., via `GlobalHeap`).

use super::generic::HeapEnvironment;
use core::mem::{align_of, size_of};

/// Size of one block header in bytes.
pub(crate) const HEADER_SIZE: usize = size_of::<HeapBlockHeader>();

/// Size of intrusive node stored in payload of free blocks.
pub(crate) const FREE_NODE_SIZE: usize = size_of::<FreeListNode>();

/// Global heap payload alignment.
pub(crate) const ALIGNMENT: usize = align_of::<usize>();

/// Returns `value` rounded up to `align` (power-of-two).
pub(crate) const fn align_up_const(value: usize, align: usize) -> usize {
    (value + (align - 1)) & !(align - 1)
}

/// Minimum free block size that can hold header + intrusive node.
pub(crate) const MIN_FREE_BLOCK_SIZE: usize =
    align_up_const(HEADER_SIZE + FREE_NODE_SIZE, ALIGNMENT);

/// Minimum tail size that is still worth splitting into a new free block.
pub(crate) const MIN_SPLIT_SIZE: usize = MIN_FREE_BLOCK_SIZE;

/// Number of segregated free-list bins.
pub(crate) const FREE_BIN_COUNT: usize = 32;

/// Virtual start address of the kernel heap arena.
pub(crate) const HEAP_START_OFFSET: usize = 0xFFFF_8000_0050_0000;

/// Heap size after `init()`.
pub(crate) const INITIAL_HEAP_SIZE: usize = 0x1000;

/// Increment used when extending the heap arena.
pub(crate) const HEAP_GROWTH: usize = 0x1000;

/// Minimum PMM headroom kept outside the heap cap for non-heap consumers.
pub(crate) const SYSTEM_HEAP_RESERVE_MIN_BYTES: usize = 8 * 1024 * 1024;

/// LSB encodes allocation state in `size_and_flags`.
pub(crate) const IN_USE_MASK: usize = 0x1;

/// Remaining bits encode block size.
pub(crate) const SIZE_MASK: usize = !IN_USE_MASK;

/// Per-header salt used to derive an address-bound validation magic.
pub(crate) const HEADER_MAGIC_SALT: usize = 0x4B41_4F53_4845_4150;

/// Public alignment constant for components that prefer const access.
pub const HEAP_ALIGNMENT: usize = ALIGNMENT;

/// Per-block metadata stored directly in heap memory.
#[repr(C)]
pub(crate) struct HeapBlockHeader {
    /// Packed representation: `[size bits | in-use bit]`.
    pub(crate) size_and_flags: usize,

    /// Full size of the physically previous block.
    pub(crate) prev_size: usize,

    /// Address-bound header magic used to reject forged payload pointers.
    pub(crate) magic: usize,
}

impl HeapBlockHeader {
    /// Returns full block size in bytes (header + payload).
    #[inline]
    pub(crate) fn size(&self) -> usize {
        self.size_and_flags & SIZE_MASK
    }

    /// Updates size bits while preserving the in-use flag.
    #[inline]
    pub(crate) fn set_size(&mut self, size: usize) {
        let flags = self.size_and_flags & IN_USE_MASK;
        self.size_and_flags = flags | (size & SIZE_MASK);
    }

    /// Returns whether this block is currently allocated.
    #[inline]
    pub(crate) fn in_use(&self) -> bool {
        (self.size_and_flags & IN_USE_MASK) != 0
    }

    /// Sets or clears the in-use bit.
    #[inline]
    pub(crate) fn set_in_use(&mut self, in_use: bool) {
        if in_use {
            self.size_and_flags |= IN_USE_MASK;
        } else {
            self.size_and_flags &= SIZE_MASK;
        }
    }

    /// Returns the size of the physically previous block.
    #[inline]
    pub(crate) fn prev_size(&self) -> usize {
        self.prev_size
    }

    /// Stores the size of the physically previous block.
    #[inline]
    pub(crate) fn set_prev_size(&mut self, size: usize) {
        self.prev_size = size;
    }

    /// Returns whether the stored header magic matches the expected value.
    #[inline]
    pub(crate) fn has_valid_magic(&self, addr: usize) -> bool {
        self.magic == header_magic_for_addr(addr)
    }

    /// Stores the expected header magic for this header address.
    #[inline]
    pub(crate) fn set_magic_for_addr(&mut self, addr: usize) {
        self.magic = header_magic_for_addr(addr);
    }
}

/// Intrusive links stored in payload of free blocks.
#[repr(C)]
pub(crate) struct FreeListNode {
    /// Address of the previous free block in the same size-class bin.
    pub(crate) prev: usize,

    /// Address of the next free block in the same size-class bin.
    pub(crate) next: usize,
}

/// Mutable heap bounds guarded by the global spinlock.
pub(crate) struct HeapState {
    /// Start address of the managed heap region.
    pub(crate) heap_start: usize,

    /// End address (exclusive) of the managed heap region.
    pub(crate) heap_end: usize,

    /// Hard upper bound for total managed heap bytes derived from system memory.
    pub(crate) max_heap_size: usize,

    /// Address of the last physical block in the heap (the one whose end == `heap_end`).
    pub(crate) tail_block_addr: usize,

    /// Segregated free-list heads, grouped by block size class.
    pub(crate) free_bins: [Option<usize>; FREE_BIN_COUNT],

    /// Bit-set for non-empty bins to accelerate candidate lookup.
    pub(crate) free_bin_bitmap: u64,
}

impl HeapState {
    pub(crate) const fn new() -> Self {
        Self {
            heap_start: 0,
            heap_end: 0,
            max_heap_size: INITIAL_HEAP_SIZE,
            tail_block_addr: 0,
            free_bins: [None; FREE_BIN_COUNT],
            free_bin_bitmap: 0,
        }
    }

    pub(crate) fn reset_free_bins(&mut self) {
        self.free_bins = [None; FREE_BIN_COUNT];
        self.free_bin_bitmap = 0;
    }
}

/// Aligns `value` up to the next `align` boundary.
#[inline]
pub(crate) fn align_up_checked(value: usize, align: usize) -> Option<usize> {
    let mask = align.checked_sub(1)?;
    value.checked_add(mask).map(|v| v & !mask)
}

/// Computes the full aligned block size for a payload request.
///
/// The returned size is clamped to [`MIN_FREE_BLOCK_SIZE`] so that every
/// allocated block can later be inserted back into the segregated free list.
/// Blocks smaller than that would silently leak on `free()` because
/// `insert_free_block` / `remove_free_block` ignore sub-minimum sizes.
#[inline]
pub(crate) fn compute_aligned_heapblock_size(requested_size: usize) -> Option<usize> {
    requested_size
        .checked_add(HEADER_SIZE)
        .and_then(|v| align_up_checked(v, ALIGNMENT))
        .map(|v| v.max(MIN_FREE_BLOCK_SIZE))
}

/// Reinterprets an address as a mutable block-header pointer.
#[inline]
pub(crate) fn header_at(addr: usize) -> *mut HeapBlockHeader {
    addr as *mut HeapBlockHeader
}

/// Converts a block header pointer to the corresponding payload pointer.
#[inline]
pub(crate) fn payload_ptr(block: *mut HeapBlockHeader) -> *mut u8 {
    block.cast::<u8>().wrapping_add(HEADER_SIZE)
}

/// Converts a free-block header pointer to its intrusive free-list node.
#[inline]
pub(crate) fn free_node_ptr(block: *mut HeapBlockHeader) -> *mut FreeListNode {
    payload_ptr(block).cast::<FreeListNode>()
}

/// Computes a deterministic, address-bound header magic.
#[inline]
pub(crate) fn header_magic_for_addr(addr: usize) -> usize {
    HEADER_MAGIC_SALT ^ addr.rotate_left(17) ^ addr.rotate_right(13)
}

/// Converts a nullable raw block pointer to an address (`0` means null).
#[inline]
pub(crate) fn ptr_to_addr(block: *mut HeapBlockHeader) -> usize {
    block as usize
}

/// Converts an address to a raw block pointer (`0` maps to null).
#[inline]
pub(crate) fn addr_to_ptr(addr: usize) -> *mut HeapBlockHeader {
    addr as *mut HeapBlockHeader
}

/// Returns the index of the bin responsible for `block_size`.
#[inline]
pub(crate) fn size_class_index(block_size: usize) -> usize {
    let normalized = block_size.max(MIN_FREE_BLOCK_SIZE);
    let log2 = (usize::BITS as usize - 1).saturating_sub(normalized.leading_zeros() as usize);
    let base =
        (usize::BITS as usize - 1).saturating_sub(MIN_FREE_BLOCK_SIZE.leading_zeros() as usize);
    let raw = log2.saturating_sub(base);
    raw.min(FREE_BIN_COUNT - 1)
}

/// Inserts a free block into its segregated bin as the list head.
pub(crate) fn insert_free_block(state: &mut HeapState, block: *mut HeapBlockHeader) {
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
pub(crate) fn remove_free_block(state: &mut HeapState, block: *mut HeapBlockHeader) {
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
pub(crate) fn find_suitable_free_block(
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

/// Validates `ptr` as a payload address returned by `malloc` and returns its block header.
///
/// Every valid payload is exactly `HEADER_SIZE` bytes past its block header, so the
/// block address is `ptr - HEADER_SIZE` — no heap walk required (O(1)).
///
/// Validation steps:
/// 1. Back-compute the expected block address.
/// 2. Verify the address lies within the managed heap arena.
/// 3. Read the header and verify magic, size, and local boundary-tag consistency.
///
/// The caller still checks `header.in_use()` to catch double-free.
pub(crate) fn find_block_by_payload_ptr(
    state: &HeapState,
    ptr: *mut u8,
) -> Option<*mut HeapBlockHeader> {
    let ptr_addr = ptr as usize;

    // Step 1: every valid payload is exactly HEADER_SIZE bytes past its block header.
    let block_addr = ptr_addr.checked_sub(HEADER_SIZE)?;

    // Step 2: block header must lie within the managed heap arena.
    if block_addr < state.heap_start || block_addr >= state.heap_end {
        return None;
    }

    // Step 3: read header and reject forged/corrupt metadata before trusting size.
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `block_addr` is within `[heap_start, heap_end)`, which is valid mapped heap memory.
    let header = unsafe { &*header_at(block_addr) };

    if !header.has_valid_magic(block_addr) {
        return None;
    }

    let block_size = header.size();

    if block_size < HEADER_SIZE || !block_size.is_multiple_of(ALIGNMENT) {
        return None;
    }

    let block_end = block_addr.checked_add(block_size)?;

    if block_end > state.heap_end {
        return None;
    }

    // Step 4: when a physical successor exists, `prev_size` must match this block size.
    let next_addr = block_end;
    if next_addr < state.heap_end {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - `next_addr` points to the immediate physical successor header in-bounds.
        let next_header = unsafe { &*header_at(next_addr) };
        if next_header.prev_size() != block_size {
            return None;
        }
    }

    Some(header_at(block_addr))
}

/// Updates `prev_size` of the physical successor block, if present.
pub(crate) fn update_next_prev_size(state: &mut HeapState, block_addr: usize, block_size: usize) {
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
pub(crate) fn allocate_block(
    state: &mut HeapState,
    block: *mut HeapBlockHeader,
    size: usize,
) -> *mut u8 {
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
            tail_header.set_magic_for_addr(tail_addr);

            // Step 2: Repair successor boundary-tag and insert split remainder.
            update_next_prev_size(state, tail_addr, tail_header.size());
            insert_free_block(state, header_at(tail_addr));

            // Step 3: If the split block was the current tail, the remainder is the new tail.
            if block as usize == state.tail_block_addr {
                state.tail_block_addr = tail_addr;
            }
        } else {
            // Step 3: Consume full block when split remainder would be too small.
            header.set_in_use(true);
            update_next_prev_size(state, block as usize, old_size);
        }
    }

    payload_ptr(block)
}

/// Coalesces `block` with physically adjacent free neighbors and returns the merged block.
pub(crate) fn coalesce_free_block(
    state: &mut HeapState,
    block: *mut HeapBlockHeader,
) -> *mut HeapBlockHeader {
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

    // Step 4: Repair the cached tail pointer when the merge changed the last
    // physical block.  If the merged block ends at `heap_end`, its start is
    // now the tail — the previous `tail_block_addr` may point into the
    // *interior* of the merged block (e.g. the absorbed old tail's header).
    // Leaving it stale makes the next `grow_heap` read a leftover header
    // there and stamp a wrong `prev_size` onto the growth block, which later
    // coalesces into overlapping free blocks and corrupts the free lists.
    if (coalesced as usize).saturating_add(final_size) == state.heap_end {
        state.tail_block_addr = coalesced as usize;
    }

    coalesced
}

/// Appends a new free block at the current heap end and coalesces neighbors.
///
/// Requires the global heap lock (`HEAP.inner`) to already be held by the caller
/// via `with_heap`, i.e. this function must only run with exclusive heap access.
pub(crate) fn grow_heap(state: &mut HeapState, amount: usize, env: &impl HeapEnvironment) -> bool {
    if amount < HEADER_SIZE {
        return false;
    }

    let current_size = state.heap_end.saturating_sub(state.heap_start);
    if current_size >= state.max_heap_size {
        return false;
    }

    let remaining = state.max_heap_size - current_size;
    if amount > remaining {
        return false;
    }

    let old_end = state.heap_end;
    let Some(new_end) = old_end.checked_add(amount) else {
        return false;
    };

    // Step 0: Ensure memory for the grown range is mapped and backed by the environment.
    if !env.map_memory(old_end, new_end) {
        return false;
    }

    // Step 1: Read the tail block size in O(1) from the cached `tail_block_addr`.
    // When the heap is empty (old_end == heap_start), there is no predecessor.
    let prev_block_size = if old_end == state.heap_start {
        0
    } else {
        // SAFETY:
        // - This requires `unsafe` because it dereferences raw pointers.
        // - `tail_block_addr` is always maintained to point to the last physical block
        //   in the heap, so dereferencing it yields a valid `HeapBlockHeader`.
        let header = unsafe { &*header_at(state.tail_block_addr) };
        let size = header.size();
        if size < HEADER_SIZE {
            return false;
        }

        // Defensive invariant check: the tail block must end exactly at the
        // old heap end.  A mismatch means `tail_block_addr` is stale and the
        // header just read is not the real last block.  Stamping its size as
        // `prev_size` would let the new block coalesce backwards into the
        // interior of another block — the corruption this guards against.
        // Degrade gracefully: `prev_size = 0` (< HEADER_SIZE) disables the
        // backward merge for this growth step, which only costs coalescing
        // opportunity, never correctness.
        debug_assert!(
            state.tail_block_addr.saturating_add(size) == old_end,
            "grow_heap: stale tail_block_addr {:#x} (size {:#x}, heap_end {:#x})",
            state.tail_block_addr,
            size,
            old_end,
        );
        if state.tail_block_addr.saturating_add(size) != old_end {
            0
        } else {
            size
        }
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
        header.set_magic_for_addr(old_end);
    }

    state.heap_end = new_end;

    // Step 3: Coalesce with neighboring free tail and enqueue once.
    let block = coalesce_free_block(state, header_at(old_end));
    // The coalesced block ends at `new_end` and is now the last physical block.
    state.tail_block_addr = block as usize;
    insert_free_block(state, block);

    true
}

#[inline]
pub(crate) fn compute_heap_growth_for_request(required_block_size: usize) -> usize {
    align_up_checked(required_block_size, HEAP_GROWTH).unwrap_or(HEAP_GROWTH)
}
