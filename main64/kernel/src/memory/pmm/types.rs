//! Type definitions, constants, and low-level helpers for the physical memory manager.

/// Size of a single page frame in bytes.
pub use crate::arch::constants::PAGE_SIZE_U64 as PAGE_SIZE;

/// Physical address where the kernel is loaded (1 MB)
pub const KERNEL_OFFSET: u64 = 0x100000;

/// Physical address of the stack top (end of reserved stack area)
pub const STACK_TOP: u64 = 0x400000;

/// Virtual base address of the kernel in the higher half
pub const KERNEL_VIRT_BASE: u64 = 0xFFFF800000000000;

#[inline]
/// Aligns `x` up to the next `align` boundary.
pub fn align_up(x: u64, align: u64) -> u64 {
    (x + align - 1) & !(align - 1)
}

#[inline]
/// Converts a kernel virtual address into a physical address.
pub fn virt_to_phys(addr: u64) -> u64 {
    if addr >= KERNEL_VIRT_BASE {
        addr - KERNEL_VIRT_BASE
    } else {
        addr
    }
}

/// Represents an allocated page frame with its PFN and region info.
/// This handle is returned by `alloc_frame`.
pub struct PageFrame {
    /// Page Frame Number (physical address / PAGE_SIZE)
    pub pfn: u64,

    /// Internal: index of the memory region this frame belongs to.
    /// Kept for future PMM diagnostics and debugging capabilities.
    #[cfg_attr(not(test), allow(dead_code))]
    pub region_index: u32,
}

impl PageFrame {
    /// Returns the physical address of this page frame.
    #[inline]
    pub fn physical_address(&self) -> u64 {
        self.pfn * PAGE_SIZE
    }
}

#[repr(C)]
/// Header for the PMM layout stored in physical memory
pub struct PmmLayoutHeader {
    /// Number of usable memory regions described by the PMM
    pub region_count: u32,

    /// Keeps the following region array 8-byte aligned
    pub padding: u32,

    /// Pointer to the first PmmRegion entry
    pub regions_ptr: *mut PmmRegion,
}

#[repr(C)]
/// Per-region metadata for the physical memory manager
pub struct PmmRegion {
    /// Physical start address of the region
    pub start: u64,

    /// Total number of page frames in this region
    pub frames_total: u64,

    /// Current number of free page frames in this region
    pub frames_free: u64,

    /// Physical address of the bitmap for this region
    pub bitmap_start: u64,

    /// Size of the bitmap in bytes (aligned to 8)
    pub bitmap_bytes: u64,

    /// Physical address of the per-frame refcount array for this region.
    ///
    /// One `u8` per frame, indexed the same way as the bitmap (`bit_idx` /
    /// frame offset from `start`). A freshly allocated frame (via
    /// [`alloc_frame`](super::manager::PhysicalMemoryManager::alloc_frame))
    /// starts at refcount `1`. Additional owners (e.g. a physical frame
    /// aliased into more than one virtual mapping) call
    /// [`inc_refcount`](super::manager::PhysicalMemoryManager::inc_refcount)
    /// to bump it. [`release_pfn`](super::manager::PhysicalMemoryManager::release_pfn)
    /// only actually frees the frame (clears the bitmap bit) once the count
    /// reaches zero.
    pub refcount_start: u64,

    /// Size of the refcount array in bytes (aligned to 8; one byte per frame).
    pub refcount_bytes: u64,
}

#[inline]
/// Sets a single bit in the PMM bitmap.
///
/// # Safety
/// `base` must point to a valid writable bitmap word array and `idx` must be
/// within the bitmap range.
pub unsafe fn set_bit(idx: u64, base: *mut u64) {
    // SAFETY:
    // - Caller must guarantee that `base` is valid for writes of `u64` at `idx / 64`.
    // - The memory range must be owned by the PMM.
    unsafe {
        let word = base.add((idx / 64) as usize);
        let mask = 1u64 << (idx % 64);
        *word |= mask;
    }
}

#[inline]
#[cfg_attr(not(test), allow(dead_code))]
/// Clears a single bit in the PMM bitmap.
///
/// # Safety
/// `base` must point to a valid writable bitmap word array and `idx` must be
/// within the bitmap range.
pub unsafe fn clear_bit(idx: u64, base: *mut u64) {
    // SAFETY:
    // - Caller must guarantee that `base` is valid for writes of `u64` at `idx / 64`.
    // - The memory range must be owned by the PMM.
    unsafe {
        let word = base.add((idx / 64) as usize);
        let mask = 1u64 << (idx % 64);
        *word &= !mask;
    }
}
