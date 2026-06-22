//! PMM on the UEFI path — synthetic-memory-map integration test.
//!
//! On the UEFI path the PMM does not read the BIOS data block; it parses the unified
//! `BootInfo` memory map and places its metadata/bitmaps in the bootloader-reserved
//! `pmm_metadata_base` region (see `docs/pmm.md` §2/§4). This test exercises that path
//! without real firmware by publishing a *synthetic* `BootInfo` (via `BOOT_INFO_PTR`)
//! before `pmm::init`, pointing at a hand-built memory map and a page-aligned static
//! buffer for the metadata region.
//!
//! It pins the behaviour the page-table/PMM rework introduced:
//! - the usable filter `is_usable && start >= KERNEL_OFFSET`,
//! - the parsed region count / `start` / `frames_total`,
//! - that bitmaps start zeroed (every non-reserved frame is free), and
//! - the *two separate* reserved ranges on the UEFI path (the low kernel+stack block and
//!   the far-away metadata region) — with the large gap between them left allocatable.
//!
//! The frame addresses in the synthetic map are not backed by real RAM, so allocated
//! frames are inspected (PFN/address) but never dereferenced.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::Ordering;

use kaos_kernel::boot_info::{
    BootInfo, FramebufferInfo, UnifiedMemoryEntry, VideoModeType, BOOT_INFO_PTR,
};
use kaos_kernel::memory::pmm::{self, KERNEL_OFFSET, PAGE_SIZE, STACK_TOP};

/// ASCII "KAOS_BOO", the agreed BootInfo sanity signature (see boot_info_layout_test).
const BOOT_MAGIC: u64 = 0x4B41_4F53_5F42_4F4F;

/// Size of the synthetic main usable region (16 MiB) — big enough to contain the entire
/// `[KERNEL_OFFSET, STACK_TOP)` reservation and still leave a large allocatable gap above it.
const MAIN_REGION_SIZE: u64 = 0x0100_0000;
/// Size of the synthetic metadata-backing region (256 KiB).
const META_REGION_SIZE: u64 = 0x0004_0000;

/// Page-aligned backing store for the PMM layout (header + region array + bitmaps).
/// `BootInfo.pmm_metadata_base` points here; its address doubles as the start of the
/// synthetic "metadata" RAM region in the memory map below.
#[repr(C, align(4096))]
struct PageAlignedBuf([u8; 8192]);
static mut META_BUF: PageAlignedBuf = PageAlignedBuf([0u8; 8192]);

/// Synthetic unified memory map handed to the PMM in place of firmware data.
static mut MMAP: [UnifiedMemoryEntry; 4] = [UnifiedMemoryEntry {
    start: 0,
    size: 0,
    is_usable: false,
}; 4];

/// Synthetic BootInfo published via `BOOT_INFO_PTR` before `pmm::init`.
static mut SYN_BOOT_INFO: BootInfo = BootInfo {
    magic: 0,
    video_type: VideoModeType::VgaText,
    fb_info: FramebufferInfo {
        base_address: 0,
        size: 0,
        width: 0,
        height: 0,
        pixels_per_scanline: 0,
    },
    memory_map_addr: 0,
    memory_map_len: 0,
    kernel_size: 0,
    pmm_metadata_base: 0,
    pmm_metadata_size: 0,
};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Address of the page-aligned metadata buffer; also the start of the synthetic
    // metadata RAM region. Reachable as a normal mapped address, so the PMM can write
    // its header/bitmaps there directly (mirrors physical==virtual on the UEFI path).
    let meta_addr = addr_of!(META_BUF) as u64;

    // Build a synthetic memory map with a deliberate mix that exercises the usable
    // filter (`is_usable && start >= KERNEL_OFFSET`):
    //   [0]  usable, below KERNEL_OFFSET   -> filtered out (start < KERNEL_OFFSET)
    //   [1]  usable, [KERNEL_OFFSET, +16M) -> KEPT  => region 0 (holds the low reservation)
    //   [2]  NON-usable, above region 0    -> filtered out (not usable)
    //   [3]  usable, [meta_addr, +256K)    -> KEPT  => region 1 (holds the PMM metadata)
    // SAFETY: single-threaded boot context; we are the only writer of these statics,
    // before they are published to the PMM.
    unsafe {
        let mmap = &mut *addr_of_mut!(MMAP);
        mmap[0] = UnifiedMemoryEntry { start: 0, size: KERNEL_OFFSET, is_usable: true };
        mmap[1] = UnifiedMemoryEntry { start: KERNEL_OFFSET, size: MAIN_REGION_SIZE, is_usable: true };
        mmap[2] = UnifiedMemoryEntry {
            start: KERNEL_OFFSET + MAIN_REGION_SIZE,
            size: 0x0010_0000,
            is_usable: false,
        };
        mmap[3] = UnifiedMemoryEntry { start: meta_addr, size: META_REGION_SIZE, is_usable: true };

        let bi = &mut *addr_of_mut!(SYN_BOOT_INFO);
        bi.magic = BOOT_MAGIC;
        bi.memory_map_addr = addr_of!(MMAP) as u64;
        bi.memory_map_len = 4;
        bi.pmm_metadata_base = meta_addr;
        bi.pmm_metadata_size = 8192;
    }

    // Publish the synthetic BootInfo so `PhysicalMemoryManager::new()` takes the UEFI path.
    BOOT_INFO_PTR.store(addr_of!(SYN_BOOT_INFO) as u64, Ordering::Release);

    pmm::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

// ============================================================================
// Expected derived quantities
// ============================================================================

/// Frames in the main region (16 MiB / 4 KiB).
const MAIN_REGION_FRAMES: u64 = MAIN_REGION_SIZE / PAGE_SIZE; // 4096
/// Frames in the metadata-backing region (256 KiB / 4 KiB).
const META_REGION_FRAMES: u64 = META_REGION_SIZE / PAGE_SIZE; // 64
/// Frames the low `[KERNEL_OFFSET, STACK_TOP)` reservation occupies inside the main region.
const LOW_RESERVED_FRAMES: u64 = (STACK_TOP - KERNEL_OFFSET) / PAGE_SIZE; // 768

// ============================================================================
// Tests
// ============================================================================

/// Contract: the UEFI usable filter and region parsing.
/// Given: a synthetic memory map with one usable-below-1MiB region, one non-usable region,
///        and two usable regions at/above KERNEL_OFFSET.
/// When: `pmm::init` parses it on the UEFI path.
/// Then: exactly the two usable, `>= KERNEL_OFFSET` regions survive, in map order, with the
///       expected `start` and `frames_total`.
/// Failure Impact: a broken usable filter would let the PMM hand out unusable/low memory,
///        or drop real RAM. Release-blocking.
#[test_case]
fn test_synthetic_uefi_regions_parsed() {
    let meta_addr = addr_of!(META_BUF) as u64;

    pmm::with_pmm(|mgr| {
        let regions = mgr.regions_snapshot();
        assert_eq!(
            regions.len(),
            2,
            "only the two usable regions at/above KERNEL_OFFSET must survive the filter"
        );

        assert_eq!(regions[0].start, KERNEL_OFFSET, "region 0 starts at KERNEL_OFFSET");
        assert_eq!(
            regions[0].frames_total, MAIN_REGION_FRAMES,
            "region 0 frame count"
        );

        assert_eq!(regions[1].start, meta_addr, "region 1 is the metadata-backing region");
        assert_eq!(
            regions[1].frames_total, META_REGION_FRAMES,
            "region 1 frame count"
        );
    });
}

/// Contract: bitmaps start zeroed and exactly the two UEFI-path reserved ranges are marked.
/// Given: the parsed synthetic map.
/// When: inspecting `frames_free` right after init.
/// Then: region 0 has exactly `LOW_RESERVED_FRAMES` used (the `[KERNEL_OFFSET, STACK_TOP)`
///       block) and everything else free; region 1 has exactly one page used (the metadata).
///       This proves the bitmaps were zeroed (no stray used bits) AND that the reservation is
///       two separate ranges, not one giant span swallowing the gap between them.
/// Failure Impact: a single-span reservation (the pre-rework bug) would mark almost all RAM
///        used; un-zeroed bitmaps would lose free frames. Release-blocking.
#[test_case]
fn test_synthetic_uefi_two_reserved_ranges() {
    pmm::with_pmm(|mgr| {
        let regions = mgr.regions_snapshot();

        // Region 0: only the low kernel+stack block is reserved; the 16 MiB region's
        // remaining frames stay free (i.e. the bitmap was zeroed and the reservation did
        // NOT extend up to the far-away metadata region).
        assert_eq!(
            regions[0].frames_free,
            MAIN_REGION_FRAMES - LOW_RESERVED_FRAMES,
            "region 0: only [KERNEL_OFFSET, STACK_TOP) reserved, gap above stays free"
        );

        // Region 1: only the metadata itself (header + regions + bitmaps, < 4 KiB here)
        // is reserved — exactly one page — leaving the rest of the region free.
        assert_eq!(
            regions[1].frames_free,
            META_REGION_FRAMES - 1,
            "region 1: only the one metadata page is reserved"
        );
    });
}

/// Contract: the first allocation lands in the free gap, above the reserved low block.
/// Given: the parsed synthetic map.
/// When: `alloc_frame()` is called.
/// Then: it returns a PFN inside region 0, at `STACK_TOP` (the first frame past the low
///       reservation), i.e. inside a usable region and outside both reserved ranges.
/// Failure Impact: allocating inside a reserved range would corrupt the kernel image, the
///        bootstrap stack, or the PMM metadata. Release-blocking.
#[test_case]
fn test_synthetic_uefi_first_alloc_skips_reserved() {
    let meta_addr = addr_of!(META_BUF) as u64;

    pmm::with_pmm(|mgr| {
        let frame = mgr.alloc_frame().expect("the free gap must be allocatable");
        let addr = frame.physical_address();

        // First free frame is the one immediately past the low reservation.
        assert_eq!(
            addr, STACK_TOP,
            "first allocatable frame is the first page above the reserved low block"
        );

        // Inside the main usable region...
        assert!(
            (KERNEL_OFFSET..KERNEL_OFFSET + MAIN_REGION_SIZE).contains(&addr),
            "allocated frame 0x{:x} must be inside the main usable region",
            addr
        );
        // ...and outside both reserved ranges.
        assert!(
            !(KERNEL_OFFSET..STACK_TOP).contains(&addr),
            "allocated frame must not be in the low reserved block"
        );
        assert!(
            !(meta_addr..meta_addr + PAGE_SIZE).contains(&addr),
            "allocated frame must not be in the reserved metadata page"
        );

        // Do NOT dereference: the synthetic frame address is not backed by real RAM.
        assert!(mgr.release_pfn(frame.pfn), "frame must release cleanly");
    });
}
