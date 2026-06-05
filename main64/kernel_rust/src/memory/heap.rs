//! Kernel heap manager.
//!
//! Design summary:
//! - Contiguous heap region with variable-sized blocks.
//! - Segregated free-list strategy with intrusive free nodes in free blocks.
//! - One header per block (`HeapBlockHeader`) storing `size`, `in_use` flag,
//!   `prev_size`, and an address-bound magic value for robust pointer validation.
//! - Block splitting on allocation and O(1) adjacent coalescing on free.
//! - Backed by a global spinlock for synchronized access.
//!
//! Notes:
//! - Block size includes the header itself.
//! - Payload pointer is always `header + HEADER_SIZE`.
//! - Heap growth is page-sized (`HEAP_GROWTH`) and relies on demand paging.

use core::mem::{align_of, size_of};

// Gated imports for kernel-only functionality (concurrency safety, logging, page table/physical memory mapping).
// This module is shared directly with user-space binaries (via `#[path]`) which do not compile with
// the `kernel` feature and lack access to Ring 0 bare-metal structures.
#[cfg(feature = "kernel")]
use {
    crate::drivers::screen::Screen,
    crate::logging,
    crate::memory::pmm,
    crate::sync::spinlock::SpinLock,
    alloc::vec::Vec,
    core::fmt::Write,
    core::sync::atomic::{AtomicBool, Ordering},
};

/// Interface for the allocator's interaction with the environment (Kernel vs. User-space).
pub trait HeapEnvironment {
    /// Informs the environment that the heap needs to grow, ensuring the virtual address range
    /// `[start, end)` is mapped and backed by physical memory.
    /// Returns `true` if successful.
    fn map_memory(&self, start: usize, end: usize) -> bool;

    /// Returns the maximum heap size in bytes.
    fn max_heap_size(&self) -> usize;

    /// Logs a debug message.
    fn log(&self, msg: &str);
}

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

/// Minimum PMM headroom kept outside the heap cap for non-heap consumers.
const SYSTEM_HEAP_RESERVE_MIN_BYTES: usize = 8 * 1024 * 1024;

/// LSB encodes allocation state in `size_and_flags`.
const IN_USE_MASK: usize = 0x1;

/// Remaining bits encode block size.
const SIZE_MASK: usize = !IN_USE_MASK;

/// Per-header salt used to derive an address-bound validation magic.
const HEADER_MAGIC_SALT: usize = 0x4B41_4F53_4845_4150;

/// Per-block metadata stored directly in heap memory.
#[repr(C)]
struct HeapBlockHeader {
    /// Packed representation: `[size bits | in-use bit]`.
    size_and_flags: usize,

    /// Full size of the physically previous block.
    prev_size: usize,

    /// Address-bound header magic used to reject forged payload pointers.
    magic: usize,
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

    /// Returns whether the stored header magic matches the expected value.
    #[inline]
    fn has_valid_magic(&self, addr: usize) -> bool {
        self.magic == header_magic_for_addr(addr)
    }

    /// Stores the expected header magic for this header address.
    #[inline]
    fn set_magic_for_addr(&mut self, addr: usize) {
        self.magic = header_magic_for_addr(addr);
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

    /// Hard upper bound for total managed heap bytes derived from system memory.
    max_heap_size: usize,

    /// Address of the last physical block in the heap (the one whose end == `heap_end`).
    ///
    /// Maintained by `init`, `grow_heap`, `allocate_block`, and `free` so that
    /// `grow_heap` can read the tail block size in O(1) instead of walking the heap.
    tail_block_addr: usize,

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
            max_heap_size: INITIAL_HEAP_SIZE,
            tail_block_addr: 0,
            free_bins: [None; FREE_BIN_COUNT],
            free_bin_bitmap: 0,
        }
    }

    fn reset_free_bins(&mut self) {
        self.free_bins = [None; FREE_BIN_COUNT];
        self.free_bin_bitmap = 0;
    }
}

#[cfg(feature = "kernel")]
pub struct KernelHeapEnv;

#[cfg(feature = "kernel")]
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

/// Compiled but unused in kernel builds; actively used in user-space heap.
#[allow(dead_code)]
/// A generic, unsynchronized Heap allocator instance.
///
/// This can be used in Ring 3 or in other custom allocator contexts by parameterizing
/// it with a custom [`HeapEnvironment`].
pub struct Heap<E: HeapEnvironment> {
    state: HeapState,
    env: E,
}

/// Compiled but unused in kernel builds; actively used in user-space heap.
#[allow(dead_code)]
impl<E: HeapEnvironment> Heap<E> {
    /// Creates a new, uninitialized heap instance with the given environment.
    pub const fn new(env: E) -> Self {
        Self {
            state: HeapState::new(),
            env,
        }
    }

    /// Initializes the heap arena at `start_addr` with `initial_size`.
    ///
    /// This will request mapping from the environment and initialize the first free block.
    pub fn init(&mut self, start_addr: usize, initial_size: usize) -> Result<(), &'static str> {
        let heap_end = start_addr
            .checked_add(initial_size)
            .ok_or("heap size overflow")?;
        let max_size = self.env.max_heap_size();

        if !self.env.map_memory(start_addr, heap_end) {
            return Err("Failed to map initial heap memory");
        }

        // SAFETY:
        // - Caller must ensure `start_addr` is valid and aligned.
        // - Environment successfully mapped the memory.
        unsafe {
            core::ptr::write_bytes(start_addr as *mut u8, 0, initial_size);
        }

        self.state.heap_start = start_addr;
        self.state.heap_end = heap_end;
        self.state.max_heap_size = max_size;
        self.state.reset_free_bins();

        // SAFETY:
        // - Initial block header is within the mapped area.
        unsafe {
            let header = &mut *header_at(start_addr);
            header.set_in_use(false);
            header.set_size(initial_size);
            header.set_prev_size(0);
            header.set_magic_for_addr(start_addr);
        }

        insert_free_block(&mut self.state, header_at(start_addr));
        self.state.tail_block_addr = start_addr;

        Ok(())
    }

    /// Allocates a block of memory satisfying `layout`.
    pub fn allocate(&mut self, layout: core::alloc::Layout) -> *mut u8 {
        let size = layout.size().max(1);
        let align = layout.align();

        // Standard alignment-aware malloc pattern:
        // If alignment exceeds ALIGNMENT, we overallocate and align up.
        if align <= ALIGNMENT {
            return self.malloc_internal(size);
        }

        let overhead = match align
            .checked_sub(1)
            .and_then(|v| v.checked_add(size_of::<*mut *mut u8>()))
        {
            Some(v) => v,
            None => return core::ptr::null_mut(),
        };
        let total_size = match size.checked_add(overhead) {
            Some(v) => v,
            None => return core::ptr::null_mut(),
        };

        let raw_ptr = self.malloc_internal(total_size);
        if raw_ptr.is_null() {
            return core::ptr::null_mut();
        }

        let aligned_addr =
            (raw_ptr as usize + size_of::<*mut *mut u8>() + align - 1) & !(align - 1);
        let aligned_ptr = aligned_addr as *mut u8;

        // SAFETY:
        // - One pointer slot before the aligned payload stores the original raw pointer.
        unsafe {
            let backref = aligned_ptr.sub(size_of::<*mut *mut u8>()).cast::<*mut u8>();
            core::ptr::write(backref, raw_ptr);
        }

        aligned_ptr
    }

    /// Frees an allocated pointer.
    ///
    /// # Safety
    /// - `ptr` must be a valid pointer previously allocated by this allocator.
    /// - `layout` must match the layout used when allocating `ptr`.
    pub unsafe fn deallocate(&mut self, ptr: *mut u8, layout: core::alloc::Layout) {
        if ptr.is_null() {
            return;
        }

        if layout.align() <= ALIGNMENT {
            let _ = self.free_internal(ptr);
            return;
        }

        // SAFETY:
        // - Read the original raw pointer stored before the aligned address.
        unsafe {
            let backref = ptr.sub(size_of::<*mut *mut u8>()).cast::<*mut u8>();
            let raw_ptr = core::ptr::read(backref);
            let _ = self.free_internal(raw_ptr);
        }
    }

    fn malloc_internal(&mut self, size: usize) -> *mut u8 {
        let Some(aligned_size) = compute_aligned_heapblock_size(size) else {
            self.env.log("alloc failed (overflow)");
            return core::ptr::null_mut();
        };

        loop {
            if let Some(block) = find_suitable_free_block(&mut self.state, aligned_size) {
                return allocate_block(&mut self.state, block, aligned_size);
            }

            let growth = compute_heap_growth_for_request(aligned_size);
            if !grow_heap(&mut self.state, growth, &self.env) {
                self.env.log("alloc failed (grow)");
                return core::ptr::null_mut();
            }
        }
    }

    fn free_internal(&mut self, ptr: *mut u8) -> Result<(), &'static str> {
        let Some(block) = find_block_by_payload_ptr(&self.state, ptr) else {
            return Err("invalid pointer");
        };

        // SAFETY:
        // - `block` points to a validated block header.
        let header = unsafe { &mut *block };
        if !header.in_use() {
            return Err("double free");
        }
        if !header.has_valid_magic(block as usize) {
            return Err("invalid magic");
        }

        header.set_in_use(false);
        let coalesced = coalesce_free_block(&mut self.state, block);
        insert_free_block(&mut self.state, coalesced);

        Ok(())
    }
}

#[cfg(feature = "kernel")]
/// Global heap singleton.
struct GlobalHeap {
    /// Protected mutable heap state.
    inner: SpinLock<HeapState>,

    /// Set to `true` after `init()` completed.
    initialized: AtomicBool,

    /// Controls whether heap logs are emitted to serial output.
    serial_debug_enabled: AtomicBool,
}

#[cfg(feature = "kernel")]
impl GlobalHeap {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(HeapState::new()),
            initialized: AtomicBool::new(false),
            serial_debug_enabled: AtomicBool::new(false),
        }
    }
}

#[cfg(feature = "kernel")]
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

#[cfg(feature = "kernel")]
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

/// Computes a deterministic, address-bound header magic.
#[inline]
fn header_magic_for_addr(addr: usize) -> usize {
    HEADER_MAGIC_SALT ^ addr.rotate_left(17) ^ addr.rotate_right(13)
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

#[cfg(feature = "kernel")]
/// Executes a closure with exclusive mutable access to heap state.
fn with_heap<R>(f: impl FnOnce(&mut HeapState) -> R) -> R {
    let mut guard = HEAP.inner.lock();
    f(&mut guard)
}

#[cfg(feature = "kernel")]
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

#[cfg(feature = "kernel")]
#[inline]
fn serial_debug_enabled() -> bool {
    HEAP.serial_debug_enabled.load(Ordering::Acquire)
}

#[cfg(feature = "kernel")]
/// Emits heap free-path diagnostics via the central logger.
#[inline]
fn log_free_diagnostic(args: core::fmt::Arguments<'_>) {
    // Step 1: Keep free-path logging on the non-allocating logger primitives only.
    // `logln_with_options` writes to serial and an in-memory fixed capture buffer,
    // so this diagnostic path does not require heap allocations.
    logging::logln_with_options("heap", args, serial_debug_enabled(), true);
}

#[cfg(feature = "kernel")]
/// Returns whether heap debug output to serial is enabled.
#[cfg_attr(not(test), allow(dead_code))]
pub fn debug_output_enabled() -> bool {
    serial_debug_enabled()
}

#[cfg(feature = "kernel")]
/// Enables or disables heap debug output and returns the previous setting.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_debug_output(enabled: bool) -> bool {
    HEAP.serial_debug_enabled.swap(enabled, Ordering::AcqRel)
}

#[cfg(feature = "kernel")]
/// Returns whether the heap manager has been initialized.
pub fn is_initialized() -> bool {
    HEAP.initialized.load(Ordering::Acquire)
}

#[cfg(feature = "kernel")]
/// Returns the current heap growth cap in bytes.
#[cfg_attr(not(test), allow(dead_code))]
pub fn max_heap_size() -> usize {
    with_heap(|state| state.max_heap_size)
}

/// Public alignment constant for components that prefer const access.
pub const HEAP_ALIGNMENT: usize = ALIGNMENT;

#[cfg(feature = "kernel")]
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

#[cfg(feature = "kernel")]
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
fn find_block_by_payload_ptr(state: &HeapState, ptr: *mut u8) -> Option<*mut HeapBlockHeader> {
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
fn grow_heap(state: &mut HeapState, amount: usize, env: &impl HeapEnvironment) -> bool {
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
        size
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
fn compute_heap_growth_for_request(required_block_size: usize) -> usize {
    align_up_checked(required_block_size, HEAP_GROWTH).unwrap_or(HEAP_GROWTH)
}

#[cfg(feature = "kernel")]
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
