/*
                    PHYSICAL MEMORY LAYOUT
    ═══════════════════════════════════════════════════════════════════

    0x00000000 ┌─────────────────────────────────────────────────────┐
               │                                                     │
               │              Real Mode Memory                       │
               │         (IVT, BDA, BIOS Data, etc.)                 │
               │                                                     │
    0x00001000 ├─────────────────────────────────────────────────────┤
               │  BiosInformationBlock (BIB_OFFSET)                  │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ year: i32                                   │    │
               │  │ month: i16                                  │    │
               │  │ day: i16                                    │    │
               │  │ hour: i16                                   │    │
               │  │ minute: i16                                 │    │
               │  │ second: i16                                 │    │
               │  │ memory_map_entries: i16                     │    │
               │  │ max_memory: i64                             │    │
               │  │ available_page_frames: i64                  │    │
               │  │ physical_memory_layout: *mut ──────────────────────┐
               │  └─────────────────────────────────────────────┘    │ │
    0x00001200 ├─────────────────────────────────────────────────────┤ │
               │  BIOS Memory Map (MEMORYMAP_OFFSET)                 │ │
               │  ┌─────────────────────────────────────────────┐    │ │
               │  │ BiosMemoryRegion[0]                         │    │ │
               │  │   start: u64                                │    │ │
               │  │   size: u64                                 │    │ │
               │  │   region_type: u32                          │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ BiosMemoryRegion[1]                         │    │ │
               │  │   ...                                       │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ BiosMemoryRegion[N]                         │    │ │
               │  │   ...                                       │    │ │
               │  └─────────────────────────────────────────────┘    │ │
               │                                                     │ │
               │         ... (reserved/other BIOS areas) ...         │ │
               │                                                     │ │
    0x00100000 ├═════════════════════════════════════════════════════┤ │
  (1 MB)       │                                                     │ │
 KERNEL_OFFSET │                    KERNEL CODE                      │ │
               │                                                     │ │
               │              (.text, .rodata, .data, .bss)          │ │
               │                                                     │ │
               │                   kernel_size bytes                 │ │
               │                                                     │ │
    0x00100000 ├─────────────────────────────────────────────────────┤ │
        +      │                                                     │ │
  kernel_size  │            (padding to 4KB alignment)               │ │
   (aligned)   │                                                     │ │
               ├═════════════════════════════════════════════════════┤◄┘
               │                                                     │
               │          PmmLayoutHeader + PmmRegion[]              │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ region_count: u32 (4 bytes)                 │    │
               │  │ padding: u32 (4 bytes)                      │    │
               │  ├─────────────────────────────────────────────┤    │
               │  │ regions[0]: PmmRegion                       │    │
               │  │   ┌───────────────────────────────────────┐ │    │
               │  │   │ start: u64                            │ │    │
               │  │   │ frames_total: u64                     │ │    │
               │  │   │ frames_free: u64                      │ │    │
               │  │   │ bitmap_start: u64 ─────────────────────────────┐
               │  │   │ bitmap_bytes: u64                     │ │      │
               │  │   └───────────────────────────────────────┘ │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ regions[1]: PmmRegion                       │    │ │
               │  │   (same fields as above) ─────────────────────────────┐
               │  ├─────────────────────────────────────────────┤    │ │  │
               │  │                ...                          │    │ │  │
               │  └─────────────────────────────────────────────┘    │ │  │
               ├═══════════════════════════════════════════════════┤◄──┘  │
               │                                                     │    │
               │         BITMAP FOR REGION 0                         │    │
               │  ┌─────────────────────────────────────────────┐    │    │
               │  │ Each bit = 1 page frame (4KB)               │    │    │
               │  │                                             │    │    │
               │  │ Word 0: [bits 0-63]                         │    │    │
               │  │   bit 0: 1=used, 0=free (PFN 0 of region)   │    │    │
               │  │   bit 1: 1=used, 0=free (PFN 1 of region)   │    │    │
               │  │   ...                                       │    │    │
               │  │ Word 1: [bits 64-127]                       │    │    │
               │  │   ...                                       │    │    │
               │  │ Word N: [bits N*64 - (N+1)*64-1]            │    │    │
               │  │                                             │    │    │
               │  │ Size = bitmap_bytes                         │    │    │
               │  └─────────────────────────────────────────────┘    │    │
               ├─────────────────────────────────────────────────────┤◄───┘
               │                                                     │
               │         BITMAP FOR REGION 1                         │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ (same structure as above)                   │    │
               │  │ Size = region[1].bitmap_bytes               │    │
               │  └─────────────────────────────────────────────┘    │
               ├─────────────────────────────────────────────────────┤
               │                                                     │
               │         BITMAP FOR REGION N...                      │
               │                                                     │
               ├═════════════════════════════════════════════════════┤
               │                                                     │
               │                                                     │
               │            FREE PHYSICAL MEMORY                     │
               │                                                     │
               │      (allocatable via alloc_frame())                │
               │                                                     │
               │                                                     │
               └─────────────────────────────────────────────────────┘
               

    ═══════════════════════════════════════════════════════════════════
                   PageFrame HANDLE (returned by alloc_frame)
    ═══════════════════════════════════════════════════════════════════

         ┌────────────────────────────────────────────────────────┐
         │  PageFrame (12 bytes logical, may be padded)           │
         │  ┌──────────────────────────────────────────────────┐  │
         │  │ pfn: u64 (8 bytes)                               │  │
         │  │   Page Frame Number = phys_addr / 4096           │  │
         │  ├──────────────────────────────────────────────────┤  │
         │  │ region_index: u32 (4 bytes, private)             │  │
         │  │   Index into memory_regions[] array              │  │
         │  └──────────────────────────────────────────────────┘  │
         │                                                        │
         │  Methods:                                              │
         │    physical_address() -> pfn * 4096                    │
         └────────────────────────────────────────────────────────┘


    ═══════════════════════════════════════════════════════════════════
                              BITMAP ENCODING
    ═══════════════════════════════════════════════════════════════════

    Each u64 word in bitmap:
    ┌────┬────┬────┬────┬────┬────┬────┬────┬─────────┬────┬────┬────┐
    │ 63 │ 62 │ 61 │ 60 │ 59 │ 58 │ 57 │ 56 │   ...   │  2 │  1 │  0 │
    └────┴────┴────┴────┴────┴────┴────┴────┴─────────┴────┴────┴────┘
      │                                                            │
      │    1 = Page frame ALLOCATED (in use)                       │
      │    0 = Page frame FREE (available)                         │
      │                                                            │
      └── bit 63 = PFN (word_index * 64 + 63)                      │
                                           bit 0 = PFN (word_index * 64 + 0)

    Example: For region starting at 0x100000 (1MB):
      Word 0, Bit 0  →  PFN = 0x100000/4096 + 0   = 256   → Phys: 0x100000
      Word 0, Bit 1  →  PFN = 0x100000/4096 + 1   = 257   → Phys: 0x101000
      Word 0, Bit 63 →  PFN = 0x100000/4096 + 63  = 319   → Phys: 0x13F000
      Word 1, Bit 0  →  PFN = 0x100000/4096 + 64  = 320   → Phys: 0x140000


    ═══════════════════════════════════════════════════════════════════
                         CONCRETE MEMORY EXAMPLE
    ═══════════════════════════════════════════════════════════════════

    Assuming: kernel_size = 0x8000 (32KB), 1 memory region with 128MB

    0x00100000  ┌──────────────────────────────────┐ ◄─ KERNEL_OFFSET
                │         Kernel (32 KB)           │
    0x00108000  ├──────────────────────────────────┤ ◄─ kernel end
                │      (padding to 4KB align)      │
    0x00109000  ├──────────────────────────────────┤ ◄─ PmmLayoutHeader
                │  region_count: 1                 │    (4 bytes)
                │  padding: 0                      │    (4 bytes)
    0x00109008  │  regions[0]:                     │    (40 bytes)
                │    start: 0x100000               │
                │    frames_total: 32768           │    (128MB / 4KB)
                │    frames_free: 32768            │
                │    bitmap_start: 0x00109030 ──────────┐
                │    bitmap_bytes: 4096            │    │
    0x00109030  ├──────────────────────────────────┤◄───┘
                │                                  │
                │     Bitmap (4096 bytes)          │
                │     32768 bits = 32768 pages     │
                │                                  │
    0x0010A030  ├──────────────────────────────────┤
                │                                  │
                │     FREE MEMORY STARTS HERE      │
                │                                  │
                │    (first ~10 pages marked used  │
                │     for kernel + PMM metadata)   │
                │                                  │
    0x08000000  └──────────────────────────────────┘ ◄─ End of 128MB
    (128 MB)
*/

#[allow(unused_imports)]
use crate::drivers::screen::Screen;
use crate::memory::bios::{self, BiosInformationBlock, BiosMemoryRegion};
#[allow(unused_imports)]
use core::fmt::Write;
extern "C" {
    /// Linker-defined symbol marking the end of the kernel BSS section
    static __bss_end: u8;
}

/// Size of a single page frame in bytes
pub const PAGE_SIZE: u64 = 4096;

/// Physical address where the kernel is loaded
const KERNEL_OFFSET: u64 = 0x100000;

/// Physical marker for the first usable memory above low memory
const MARK_1MB: u64 = 0x100000;

/// Physical address of the stack top (end of reserved stack area)
const STACK_TOP: u64 = 0x400000;

/// Virtual base address of the kernel in the higher half
const KERNEL_VIRT_BASE: u64 = 0xFFFF800000000000;

#[inline]
/// Aligns `x` up to the next `align` boundary.
fn align_up(x: u64, align: u64) -> u64 {
    (x + align - 1) & !(align - 1)
}

#[inline]
/// Converts a kernel virtual address into a physical address.
fn virt_to_phys(addr: u64) -> u64 {
    if addr >= KERNEL_VIRT_BASE {
        addr - KERNEL_VIRT_BASE
    } else {
        addr
    }
}

#[inline]
/// Sets a single bit in the PMM bitmap.
unsafe fn set_bit(idx: u64, base: *mut u64) {
    let word = base.add((idx / 64) as usize);
    let mask = 1u64 << (idx % 64);
    *word |= mask;
}

#[inline]
#[allow(dead_code)]
/// Clears a single bit in the PMM bitmap.
unsafe fn clear_bit(idx: u64, base: *mut u64) {
    let word = base.add((idx / 64) as usize);
    let mask = 1u64 << (idx % 64);
    *word &= !mask;
}

/// Represents an allocated page frame with its PFN and region info.
/// This handle is returned by `alloc_frame` and passed to `release_frame`.
#[allow(dead_code)]
pub struct PageFrame {
    /// Page Frame Number (physical address / PAGE_SIZE)
    pub pfn: u64,

    /// Internal: index of the memory region this frame belongs to
    region_index: u32,
}

#[allow(dead_code)]
impl PageFrame {
    /// Returns the physical address of this page frame.
    #[inline]
    pub fn physical_address(&self) -> u64 {
        self.pfn * PAGE_SIZE
    }

    /// Returns the index of the region this frame belongs to.
    #[inline]
    pub fn region_index(&self) -> u32 {
        self.region_index
    }
}

#[repr(C)]
/// Header for the PMM layout stored in physical memory
pub struct PmmLayoutHeader {
    /// Number of usable memory regions described by the PMM
    region_count: u32,

    /// Keeps the following region array 8-byte aligned
    padding: u32,

    /// Pointer to the first PmmRegion entry
    regions_ptr: *mut PmmRegion,
}

#[repr(C)]
/// Per-region metadata for the physical memory manager
pub struct PmmRegion {
    /// Physical start address of the region
    start: u64,

    /// Total number of page frames in this region
    frames_total: u64,

    /// Current number of free page frames in this region
    frames_free: u64,

    /// Physical address of the bitmap for this region
    bitmap_start: u64,
    
    /// Size of the bitmap in bytes (aligned to 8)
    bitmap_bytes: u64,
}

/// Global PMM instance initialized by `init`.
static mut PMM: PhysicalMemoryManager = PhysicalMemoryManager {
    header: core::ptr::null_mut(),
};

/// Initializes the global physical memory manager.
pub fn init() {
    unsafe {
        PMM = PhysicalMemoryManager::new();
    }
}

/// Executes a closure with a mutable reference to the PMM instance.
#[allow(dead_code, static_mut_refs)]
pub fn with_pmm<R>(f: impl FnOnce(&mut PhysicalMemoryManager) -> R) -> R {
    unsafe {
        debug_assert!(!PMM.header.is_null(), "PMM not initialized");
        f(&mut PMM)
    }
}

/// Physical memory manager for allocating and freeing page frames.
pub struct PhysicalMemoryManager {
    /// Pointer to the PMM layout header in physical memory
    header: *mut PmmLayoutHeader,
}

impl PhysicalMemoryManager {
    /// Constructs the PMM layout and initializes region metadata.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (bib, region) = unsafe {
            (
                (bios::BIB_OFFSET as *mut BiosInformationBlock).as_mut().unwrap(),
                bios::MEMORYMAP_OFFSET as *const BiosMemoryRegion,
            )
        };

        // Place PMM layout right after the kernel image (including BSS), aligned to 4K.
        let kernel_end_virt = unsafe { &__bss_end as *const u8 as u64 };
        let kernel_end_phys = virt_to_phys(kernel_end_virt);
        let start_addr = align_up(kernel_end_phys, PAGE_SIZE);
        let header = start_addr as *mut PmmLayoutHeader;
        unsafe {
            (*header).region_count = 0;
            (*header).padding = 0;
            (*header).regions_ptr = (header as *mut u8)
                .add(core::mem::size_of::<PmmLayoutHeader>()) as *mut PmmRegion;
        }
        
        let mut pmm = Self { header };

        // Count usable regions first
        let mut count = 0u32;

        for i in 0..bib.memory_map_entries as usize {
            let r = unsafe { &*region.add(i) };
            if r.region_type == 1 && r.start >= MARK_1MB {
                count += 1;
            }
        }
        unsafe {
            (*header).region_count = count;
        }

        let regions = unsafe {
            let count = (*header).region_count as usize;
            let regions_ptr = (*header).regions_ptr;
            core::slice::from_raw_parts_mut(regions_ptr, count)
        };

        // Fill regions
        let mut idx = 0usize;

        for i in 0..bib.memory_map_entries as usize {
            let r = unsafe { &*region.add(i) };

            if r.region_type == 1 && r.start >= MARK_1MB {
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

        // Bitmaps right after the region array.
        let mut bitmap_base = (regions.as_ptr() as u64)
            + (count as u64) * (core::mem::size_of::<PmmRegion>() as u64);

        for r in regions.iter_mut() {
            r.bitmap_start = bitmap_base;
            unsafe { core::ptr::write_bytes(r.bitmap_start as *mut u8, 0, r.bitmap_bytes as usize) };
            bitmap_base += r.bitmap_bytes;
        }

        // Mark kernel + PMM metadata used.
        let used_frames = (STACK_TOP - KERNEL_OFFSET) / PAGE_SIZE;
        for _ in 0..used_frames {
            let _ = pmm.alloc_frame();
        }

        pmm
    }

    /// Allocates a single page frame from the first available region.
    /// Returns `Some(PageFrame)` on success, or `None` if no free frames exist.
    pub fn alloc_frame(&mut self) -> Option<PageFrame> {
        let regions = unsafe {
            let count = (*self.header).region_count as usize;
            let regions_ptr = (*self.header).regions_ptr;
            core::slice::from_raw_parts_mut(regions_ptr, count)
        };

        for (idx, r) in regions.iter_mut().enumerate() {
            if r.frames_free == 0 {
                continue;
            }

            let words = (r.bitmap_bytes / 8) as usize;
            let bitmap = r.bitmap_start as *mut u64;

            for w in 0..words {
                let val = unsafe { *bitmap.add(w) };

                if val != u64::MAX {
                    let free_bit = (!val).trailing_zeros() as u64;
                    let bit_idx = (w as u64) * 64 + free_bit;

                    if bit_idx < r.frames_total {
                        unsafe { set_bit(bit_idx, bitmap) };
                        r.frames_free -= 1;

                        return Some(PageFrame {
                            pfn: r.start / PAGE_SIZE + bit_idx,
                            region_index: idx as u32,
                        });
                    }
                }
            }
        }

        None
    }

    /// Releases a previously allocated page frame back to the pool.
    /// The `PageFrame` handle contains all information needed to free the frame.
    #[allow(dead_code)]
    pub fn release_frame(&mut self, frame: PageFrame) {
        let regions = unsafe {
            let count = (*self.header).region_count as usize;
            let regions_ptr = (*self.header).regions_ptr;
            core::slice::from_raw_parts_mut(regions_ptr, count)
        };

        let r = &mut regions[frame.region_index as usize];
        let bitmap = r.bitmap_start as *mut u64;
        let bit_idx = frame.pfn - (r.start / PAGE_SIZE);
        unsafe { clear_bit(bit_idx, bitmap) };
        r.frames_free += 1;
    }
}
