//! Physical memory manager implementation.

use crate::boot_info::{BootInfo, UnifiedMemoryEntry, BOOT_INFO_PTR};
use crate::memory::bios::{self, BiosInformationBlock, BiosMemoryRegion};
use super::types::{
    align_up, clear_bit, set_bit, virt_to_phys, PageFrame, PmmLayoutHeader, PmmRegion, KERNEL_OFFSET, PAGE_SIZE, STACK_TOP
};
use core::sync::atomic::Ordering;

extern "C" {
    /// Linker-defined symbol marking the end of the kernel BSS section
    static __bss_end: u8;
}

/// Physical memory manager for allocating and freeing page frames.
pub struct PhysicalMemoryManager {
    /// Pointer to the PMM layout header in physical memory
    pub(crate) header: *mut PmmLayoutHeader,
}

// SAFETY:
// - PhysicalMemoryManager contains a raw pointer to static physical memory.
// - Access is synchronized via SpinLock, and the pointer is never sent across threads unsafely.
// - The PMM layout is stable after initialization.
unsafe impl Send for PhysicalMemoryManager {}

impl PhysicalMemoryManager {
    /// Returns a mutable slice over the PMM region array stored after the header.
    fn regions(&mut self) -> &mut [PmmRegion] {
        // SAFETY:
        // - `self.header` is initialized in `new()` before this method is used.
        // - `region_count` and `regions_ptr` belong to the in-memory PMM layout.
        // - The returned mutable slice is tied to `&mut self`, preventing aliasing.
        unsafe {
            let count = (*self.header).region_count as usize;
            let regions_ptr = (*self.header).regions_ptr;
            core::slice::from_raw_parts_mut(regions_ptr, count)
        }
    }

    /// Constructs the PMM layout and initializes region metadata.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // Place PMM layout right after the kernel image (including BSS), aligned to 4K.
        // SAFETY: `__bss_end` is a linker-defined symbol with static lifetime.
        let kernel_end_virt = unsafe { &__bss_end as *const u8 as u64 };
        let kernel_end_phys = virt_to_phys(kernel_end_virt);
        let start_addr = align_up(kernel_end_phys, PAGE_SIZE);
        let header = start_addr as *mut PmmLayoutHeader;
        
        // SAFETY:
        // - `header` points into reserved physical memory owned by PMM metadata.
        // - We initialize the layout header exactly once during PMM construction.
        unsafe {
            (*header).region_count = 0;
            (*header).padding = 0;
            (*header).regions_ptr =
                (header as *mut u8).add(core::mem::size_of::<PmmLayoutHeader>()) as *mut PmmRegion;
        }

        let mut pmm = Self { header };

        // Count usable regions and initialize region array
        let mut count = 0u32;
        let boot_info_raw = BOOT_INFO_PTR.load(Ordering::Acquire);

        if boot_info_raw != 0 {
            // SAFETY:
            // - `boot_info_raw` contains a valid pointer to the unified BootInfo structure.
            // - The memory map array it references contains valid, aligned records in low memory.
            unsafe {
                let boot_info = &*(boot_info_raw as *const BootInfo);
                let entries = boot_info.memory_map_len as usize;
                let entry_ptr = boot_info.memory_map_addr as *const UnifiedMemoryEntry;

                // Step 1: Count usable regions above 1MB
                for i in 0..entries {
                    let entry = &*entry_ptr.add(i);
                    if entry.is_usable && entry.start >= KERNEL_OFFSET {
                        count += 1;
                    }
                }

                (*header).region_count = count;
                let regions = pmm.regions();
                let mut idx = 0usize;

                // Step 2: Populate the region table from the unified map
                for i in 0..entries {
                    let entry = &*entry_ptr.add(i);
                    if entry.is_usable && entry.start >= KERNEL_OFFSET {
                        let frames = entry.size / PAGE_SIZE;
                        let bitmap_bytes = align_up(frames.div_ceil(8), 8);

                        regions[idx] = PmmRegion {
                            start: entry.start,
                            frames_total: frames,
                            frames_free: frames,
                            bitmap_start: 0,
                            bitmap_bytes,
                        };
                        idx += 1;
                    }
                }
            }
        } else {
            // SAFETY:
            // - Bootloader populated BIOS data at fixed offsets before kernel entry.
            // - `BIB_OFFSET` and `MEMORYMAP_OFFSET` point to valid static data.
            let (bib, region) = unsafe {
                (
                    (bios::BIB_OFFSET as *mut BiosInformationBlock)
                        .as_mut()
                        .unwrap(),
                    bios::MEMORYMAP_OFFSET as *const BiosMemoryRegion,
                )
            };

            // Step 1: Count usable regions above 1MB
            for i in 0..bib.memory_map_entries as usize {
                // SAFETY:
                // - `i` is bounded by `memory_map_entries`.
                // - `region` points to a contiguous BIOS memory map array.
                let r = unsafe { &*region.add(i) };
                if r.region_type == 1 && r.start >= KERNEL_OFFSET {
                    count += 1;
                }
            }
            
            // SAFETY: `header` is valid and writable during PMM initialization.
            unsafe {
                (*header).region_count = count;
            }

            let regions = pmm.regions();
            let mut idx = 0usize;

            // Step 2: Populate the region table from the BIOS map
            for i in 0..bib.memory_map_entries as usize {
                // SAFETY:
                // - `i` is bounded by `memory_map_entries`.
                // - `region` points to a contiguous BIOS memory map array.
                let r = unsafe { &*region.add(i) };

                if r.region_type == 1 && r.start >= KERNEL_OFFSET {
                    let frames = r.size / PAGE_SIZE;
                    let bitmap_bytes = align_up(frames.div_ceil(8), 8);

                    regions[idx] = PmmRegion {
                        start: r.start,
                        frames_total: frames,
                        frames_free: frames,
                        bitmap_start: 0,
                        bitmap_bytes,
                    };
                    idx += 1;
                }
            }
        }

        let regions = pmm.regions();

        // Bitmaps right after the region array.
        let mut bitmap_base =
            (regions.as_ptr() as u64) + (count as u64) * (core::mem::size_of::<PmmRegion>() as u64);

        for r in regions.iter_mut() {
            r.bitmap_start = bitmap_base;
            // SAFETY:
            // - `bitmap_start..bitmap_start+bitmap_bytes` is PMM-owned metadata memory.
            // - We clear each bitmap once before allocator use.
            unsafe {
                core::ptr::write_bytes(r.bitmap_start as *mut u8, 0, r.bitmap_bytes as usize)
            };
            bitmap_base += r.bitmap_bytes;
        }

        // Mark the kernel, stack, and PMM metadata as used.
        // `bitmap_base` already points past the last bitmap, so it is the
        // true end of all PMM metadata. We take the max with STACK_TOP to
        // also cover the bootloader stack, then align up to a page boundary.
        let metadata_end = bitmap_base;
        let reserved_end = align_up(metadata_end.max(STACK_TOP), PAGE_SIZE);
        pmm.mark_range_used(KERNEL_OFFSET, reserved_end);

        pmm
    }

    /// Marks every page frame in the physical range `[range_start, range_end)`
    /// as used by directly setting the corresponding bitmap bits.
    /// This does not depend on the allocation order of `alloc_frame()`.
    fn mark_range_used(&mut self, range_start: u64, range_end: u64) {
        let regions = self.regions();

        for r in regions.iter_mut() {
            let region_end = r.start + r.frames_total * PAGE_SIZE;

            // Compute the overlap between the reserved range and this region.
            let overlap_start = range_start.max(r.start);
            let overlap_end = range_end.min(region_end);

            if overlap_start >= overlap_end {
                continue;
            }

            let first_bit = (overlap_start - r.start) / PAGE_SIZE;
            let end_bit = (overlap_end - r.start) / PAGE_SIZE;
            let bitmap = r.bitmap_start as *mut u64;

            for bit in first_bit..end_bit {
                // SAFETY:
                // - `bit` is within the region bitmap bounds derived from overlap.
                // - `bitmap` points to writable PMM bitmap memory.
                unsafe { set_bit(bit, bitmap) };
                r.frames_free -= 1;
            }
        }
    }

    /// Allocates a single page frame from the first available region.
    /// Returns `Some(PageFrame)` on success, or `None` if no free frames exist.
    pub fn alloc_frame(&mut self) -> Option<PageFrame> {
        let regions = self.regions();

        for (idx, r) in regions.iter_mut().enumerate() {
            if r.frames_free == 0 {
                continue;
            }

            let words = (r.bitmap_bytes / 8) as usize;
            let bitmap = r.bitmap_start as *mut u64;

            for w in 0..words {
                // SAFETY:
                // - `w < words` and `words == bitmap_bytes/8`, so `bitmap.add(w)` is in-bounds.
                // - `bitmap` points to PMM-owned bitmap memory.
                let val = unsafe { *bitmap.add(w) };

                if val != u64::MAX {
                    let free_bit = (!val).trailing_zeros() as u64;
                    let bit_idx = (w as u64) * 64 + free_bit;

                    if bit_idx < r.frames_total {
                        // SAFETY:
                        // - `bit_idx < frames_total` ensures valid bit index for this region.
                        // - `bitmap` points to writable PMM bitmap memory.
                        unsafe { set_bit(bit_idx, bitmap) };
                        r.frames_free -= 1;

                        // Log the allocation
                        let pfn = r.start / PAGE_SIZE + bit_idx;
                        let region_index = idx as u32;
                        super::log_alloc(pfn, region_index);

                        return Some(PageFrame { pfn, region_index });
                    }
                }
            }
        }

        None
    }

    /// Releases a page frame identified by PFN.
    ///
    /// Returns `true` when the PFN belongs to a known region and was marked used.
    /// Returns `false` when the PFN is outside all regions or already free.
    pub fn release_pfn(&mut self, pfn: u64) -> bool {
        let regions = self.regions();

        for (region_index, r) in regions.iter_mut().enumerate() {
            let region_start_pfn = r.start / PAGE_SIZE;
            let region_end_pfn = region_start_pfn + r.frames_total;
            if pfn < region_start_pfn || pfn >= region_end_pfn {
                continue;
            }

            let bit_idx = pfn - region_start_pfn;
            let bitmap = r.bitmap_start as *mut u64;
            let word_idx = (bit_idx / 64) as usize;
            let bit_mask = 1u64 << (bit_idx % 64);
            
            // SAFETY:
            // - `bit_idx` is derived from a PFN proven to be inside this region.
            // - Therefore `word_idx` addresses a valid bitmap word.
            let word_ptr = unsafe { bitmap.add(word_idx) };
            
            // SAFETY: `word_ptr` points to a valid bitmap word for this region.
            let word_val = unsafe { *word_ptr };

            if (word_val & bit_mask) == 0 {
                return false;
            }

            // SAFETY:
            // - `bit_idx` belongs to this region and currently marks an allocated frame.
            // - `bitmap` points to writable PMM bitmap memory.
            unsafe { clear_bit(bit_idx, bitmap) };
            r.frames_free += 1;
            super::log_release(pfn, region_index as u32);
            return true;
        }

        false
    }

    /// Returns the sum of currently free frames across all regions.
    pub fn total_free_frames(&mut self) -> u64 {
        let regions = self.regions();
        regions.iter().map(|r| r.frames_free).sum()
    }
}
