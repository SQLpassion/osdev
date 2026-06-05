//! Submodule defining the generic, unsynchronized allocator wrapper.
//!
//! Design summary:
//! - Defines the [`HeapEnvironment`] trait used to decouple bare-metal memory mapping
//!   and logging from the generic allocation algorithms.
//! - Defines the generic [`Heap`] struct which wraps [`HeapState`] and parameterizes
//!   its execution using an implementation of [`HeapEnvironment`].
//!
//! Use-cases:
//! - User-space programs reuse this generic allocator by providing a Ring 3 mapping environment.
//! - Integration tests instantiate this directly with mock stacks or environments.

use core::mem::size_of;
use super::types::{
    HeapState,
    compute_aligned_heapblock_size, find_suitable_free_block,
    allocate_block, compute_heap_growth_for_request, grow_heap,
    find_block_by_payload_ptr, coalesce_free_block, insert_free_block,
    header_at, ALIGNMENT,
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

/// Compiled but unused in kernel builds; actively used in user-space heap.
#[allow(dead_code)]
/// A generic, unsynchronized Heap allocator instance.
///
/// This can be used in Ring 3 or in other custom allocator contexts by parameterizing
/// it with a custom [`HeapEnvironment`].
pub struct Heap<E: HeapEnvironment> {
    /// Internal mutable state of the allocator, including bounds and free lists.
    pub(crate) state: HeapState,

    /// The environment interface supplying memory mapping and logging capabilities.
    pub(crate) env: E,
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

    /// Aligns size, searches bins, and handles heap growth logic.
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

    /// Performs pointer validation, unlinks/coalesces neighbors, and re-inserts into bins.
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
