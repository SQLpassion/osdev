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
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
extern "C" {
    /// Linker-defined symbol marking the end of the kernel BSS section
    static __bss_end: u8;
}

/// Size of a single page frame in bytes
pub const PAGE_SIZE: u64 = 4096;

/// Physical address where the kernel is loaded (1 MB)
const KERNEL_OFFSET: u64 = 0x100000;

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

/// Wrapper that holds the global PMM behind an `UnsafeCell` so we avoid
/// `static mut` (which allows aliased `&mut` references and is unsound).
/// An `AtomicBool` tracks whether `init()` has been called.
struct GlobalPmm {
    inner: UnsafeCell<PhysicalMemoryManager>,
    initialized: AtomicBool,
}

impl GlobalPmm {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(PhysicalMemoryManager {
                header: core::ptr::null_mut(),
            }),
            initialized: AtomicBool::new(false),
        }
    }
}

// Safety: The kernel is single-threaded (no SMP) and interrupt handlers never
// access the PMM.  The atomic flag provides correct Acquire/Release ordering
// so that callers of `with_pmm` observe the fully-constructed state.
unsafe impl Sync for GlobalPmm {}

static PMM: GlobalPmm = GlobalPmm::new();

/// Initializes the global physical memory manager.
pub fn init() {
    unsafe {
        *PMM.inner.get() = PhysicalMemoryManager::new();
    }
    PMM.initialized.store(true, Ordering::Release);
}

/// Executes a closure with a mutable reference to the PMM instance.
#[allow(dead_code)]
pub fn with_pmm<R>(f: impl FnOnce(&mut PhysicalMemoryManager) -> R) -> R {
    debug_assert!(PMM.initialized.load(Ordering::Acquire), "PMM not initialized");
    unsafe { f(&mut *PMM.inner.get()) }
}

/// Physical memory manager for allocating and freeing page frames.
pub struct PhysicalMemoryManager {
    /// Pointer to the PMM layout header in physical memory
    header: *mut PmmLayoutHeader,
}

impl PhysicalMemoryManager {
    /// Returns a mutable slice over the PMM region array stored after the header.
    fn regions(&mut self) -> &mut [PmmRegion] {
        unsafe {
            let count = (*self.header).region_count as usize;
            let regions_ptr = (*self.header).regions_ptr;
            core::slice::from_raw_parts_mut(regions_ptr, count)
        }
    }

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
            if r.region_type == 1 && r.start >= KERNEL_OFFSET {
                count += 1;
            }
        }
        unsafe {
            (*header).region_count = count;
        }

        let regions = pmm.regions();

        // Fill regions
        let mut idx = 0usize;

        for i in 0..bib.memory_map_entries as usize {
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

        // Bitmaps right after the region array.
        let mut bitmap_base = (regions.as_ptr() as u64)
            + (count as u64) * (core::mem::size_of::<PmmRegion>() as u64);

        for r in regions.iter_mut() {
            r.bitmap_start = bitmap_base;
            unsafe { core::ptr::write_bytes(r.bitmap_start as *mut u8, 0, r.bitmap_bytes as usize) };
            bitmap_base += r.bitmap_bytes;
        }

        // Mark the kernel, stack, and PMM metadata as used.
        // `bitmap_base` already points past the last bitmap, so it is the
        // true end of all PMM metadata.  We take the max with STACK_TOP to
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
                unsafe { set_bit(bit, bitmap) };
                r.frames_free -= 1;
            }
        }
    }

    /// Allocates a single page frame from the first available region.
    /// Returns `Some(PageFrame)` on success, or `None` if no free frames exist.
    #[allow(dead_code)]
    pub fn alloc_frame(&mut self) -> Option<PageFrame> {
        let regions = self.regions();

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
        let regions = self.regions();

        let r = &mut regions[frame.region_index as usize];
        let bitmap = r.bitmap_start as *mut u64;
        let bit_idx = frame.pfn - (r.start / PAGE_SIZE);
        unsafe { clear_bit(bit_idx, bitmap) };
        r.frames_free += 1;
    }
}

/// Runs PMM runtime self-tests and prints results to the screen.
pub fn run_self_test(screen: &mut Screen, stress_iters: u32) {
    let mut failures = 0u32;
    crate::debugln!("[pmm-test] start (stress={})", stress_iters);
    writeln!(screen, "Running PMM self-test (stress: {})...", stress_iters).unwrap();

    with_pmm(|mgr| {
        let frame0 = match mgr.alloc_frame() {
            Some(f) => f,
            None => {
                failures += 1;
                crate::debugln!("[pmm-test] FAIL alloc frame0");
                writeln!(screen, "  [FAIL] alloc frame0").unwrap();
                return;
            }
        };
        let frame1 = match mgr.alloc_frame() {
            Some(f) => f,
            None => {
                failures += 1;
                crate::debugln!("[pmm-test] FAIL alloc frame1");
                writeln!(screen, "  [FAIL] alloc frame1").unwrap();
                mgr.release_frame(frame0);
                return;
            }
        };
        let frame2 = match mgr.alloc_frame() {
            Some(f) => f,
            None => {
                failures += 1;
                crate::debugln!("[pmm-test] FAIL alloc frame2");
                writeln!(screen, "  [FAIL] alloc frame2").unwrap();
                mgr.release_frame(frame1);
                mgr.release_frame(frame0);
                return;
            }
        };
        crate::debugln!(
            "[pmm-test] allocated pfns: {}, {}, {}",
            frame0.pfn,
            frame1.pfn,
            frame2.pfn
        );

        if frame0.pfn == frame1.pfn || frame1.pfn == frame2.pfn || frame0.pfn == frame2.pfn {
            failures += 1;
            crate::debugln!("[pmm-test] FAIL unique PFNs");
            writeln!(screen, "  [FAIL] allocated PFNs are not unique").unwrap();
        } else {
            crate::debugln!("[pmm-test] OK unique PFNs");
            writeln!(screen, "  [ OK ] unique PFNs on consecutive allocations").unwrap();
        }

        let addr0 = frame0.physical_address();
        let addr1 = frame1.physical_address();
        let addr2 = frame2.physical_address();

        if addr0 % PAGE_SIZE != 0 || addr1 % PAGE_SIZE != 0 || addr2 % PAGE_SIZE != 0 {
            failures += 1;
            crate::debugln!(
                "[pmm-test] FAIL alignment: {:#x}, {:#x}, {:#x}",
                addr0,
                addr1,
                addr2
            );
            writeln!(screen, "  [FAIL] physical address alignment").unwrap();
        } else {
            crate::debugln!(
                "[pmm-test] OK alignment: {:#x}, {:#x}, {:#x}",
                addr0,
                addr1,
                addr2
            );
            writeln!(screen, "  [ OK ] physical address alignment").unwrap();
        }

        let reserved = |addr: u64| (KERNEL_OFFSET..STACK_TOP).contains(&addr);
        if reserved(addr0) || reserved(addr1) || reserved(addr2) {
            failures += 1;
            crate::debugln!(
                "[pmm-test] FAIL reserved range hit: {:#x}, {:#x}, {:#x}",
                addr0,
                addr1,
                addr2
            );
            writeln!(screen, "  [FAIL] frame allocated in reserved range").unwrap();
        } else {
            crate::debugln!("[pmm-test] OK reserved range check");
            writeln!(screen, "  [ OK ] reserved range is not allocated").unwrap();
        }

        let old_mid_pfn = frame1.pfn;
        mgr.release_frame(frame1);
        let reused = match mgr.alloc_frame() {
            Some(f) => f,
            None => {
                failures += 1;
                crate::debugln!("[pmm-test] FAIL re-allocation after release");
                writeln!(screen, "  [FAIL] re-allocation after release").unwrap();
                mgr.release_frame(frame2);
                mgr.release_frame(frame0);
                return;
            }
        };
        if reused.pfn != old_mid_pfn {
            failures += 1;
            crate::debugln!(
                "[pmm-test] FAIL reuse mismatch: expected {}, got {}",
                old_mid_pfn,
                reused.pfn
            );
            writeln!(screen, "  [FAIL] released frame was not reused first").unwrap();
        } else {
            crate::debugln!("[pmm-test] OK frame reuse ({})", reused.pfn);
            writeln!(screen, "  [ OK ] released frame is reused").unwrap();
        }

        mgr.release_frame(reused);
        mgr.release_frame(frame2);
        mgr.release_frame(frame0);

        for i in 0..stress_iters {
            let f = match mgr.alloc_frame() {
                Some(f) => f,
                None => {
                    failures += 1;
                    crate::debugln!("[pmm-test] FAIL stress alloc at iter {}", i);
                    writeln!(screen, "  [FAIL] stress alloc failed at iter {}", i).unwrap();
                    break;
                }
            };

            if f.physical_address() % PAGE_SIZE != 0 {
                failures += 1;
                crate::debugln!(
                    "[pmm-test] FAIL stress alignment at iter {} addr={:#x}",
                    i,
                    f.physical_address()
                );
                writeln!(screen, "  [FAIL] stress alignment at iter {}", i).unwrap();
                mgr.release_frame(f);
                break;
            }

            mgr.release_frame(f);

            if i != 0 && i % 512 == 0 {
                crate::debugln!("[pmm-test] stress progress: {}/{}", i, stress_iters);
            }
        }

        if failures == 0 {
            crate::debugln!("[pmm-test] OK stress {} cycles", stress_iters);
            writeln!(screen, "  [ OK ] stress {} alloc/release cycles", stress_iters).unwrap();
        }
    });

    if failures == 0 {
        crate::debugln!("[pmm-test] PASSED");
        writeln!(screen, "PMM self-test PASSED").unwrap();
    } else {
        crate::debugln!("[pmm-test] FAILED ({} issue(s))", failures);
        writeln!(screen, "PMM self-test FAILED ({} issue(s))", failures).unwrap();
    }
}
