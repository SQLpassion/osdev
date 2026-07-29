//! `direct_map::build_direct_map` / `validate_direct_map_coverage` / `build_full_kernel_pml4`
//! — pure-builder integration tests (part of #63).
//!
//! Most tests here build a synthetic PML4 over a small static pool of page-aligned
//! `PageTable`s, standing in for scaffold frames a real boot would draw from the PMM
//! (same trick as `pmm_uefi_test.rs`'s `META_BUF`: the buffer's own address is used as
//! the "physical" address, converted through `virt_to_phys` where it is written into a
//! page-table entry's frame field — see `page_table_test.rs` for why that conversion
//! is required). These tests never switch CR3 and never rely on `vmm::init`.
//!
//! The exceptions are the `build_full_kernel_pml4` tests, which draw real scaffold
//! frames from the global PMM (`pmm::init` + `page_table::alloc_frame_phys`) to build a
//! complete kernel-owned table without switching to it.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicUsize, Ordering};

use kaos_kernel::arch::constants::PAGE_SIZE_U64;
use kaos_kernel::boot_info::{
    BootInfo, FramebufferInfo, PixelFormat, UnifiedMemoryEntry, VideoModeType,
};
use kaos_kernel::memory::pmm::{self, types::virt_to_phys};
use kaos_kernel::memory::vmm::direct_map::{
    build_direct_map, build_full_kernel_pml4, build_uc_direct_map, is_loader_owned, is_mmio,
    is_phase1_ram, is_phase2_platform, map_wc_range, validate_direct_map_coverage,
    validate_essential_boot_addresses, CoverageGap, DirectMapError,
};
use kaos_kernel::memory::vmm::page_table::{
    self, pd_index, pt_index, resolve_phys_via_root, PageTable, ENTRY_PCD, ENTRY_PWT,
    HUGE_PAGE_SIZE_2M,
};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Only needed by the real-PMM free-tables test below; harmless for the others.
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

/// Fresh, page-aligned scaffold frame pool for the synthetic-table tests. 16 frames is
/// comfortably more than any single small test region needs (a fresh empty tree needs
/// at most 1 PDPT + 1 PD + 1 PT frame to map its first byte).
const POOL_SIZE: usize = 16;
static mut POOL: [PageTable; POOL_SIZE] = {
    const EMPTY: PageTable = PageTable::new();
    [EMPTY; POOL_SIZE]
};
static POOL_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Resets the pool and returns the (already zeroed) physical address of `POOL[0]`, to
/// be used as a fresh PML4 root by the caller.
///
/// # Safety
/// Caller must not run this concurrently with any other access to `POOL`.
unsafe fn reset_pool() -> u64 {
    let pool = &mut *addr_of_mut!(POOL);
    for table in pool.iter_mut() {
        table.zero();
    }
    POOL_NEXT.store(1, Ordering::Relaxed); // slot 0 reserved for the PML4 root.
    virt_to_phys(addr_of!(POOL[0]) as u64)
}

/// Bump allocator over `POOL[1..]`. Returns `None` once exhausted, exercising the same
/// `Option<u64>` contract as the real `page_table::alloc_frame_phys`.
fn bump_alloc_from_pool() -> Option<u64> {
    let idx = POOL_NEXT.fetch_add(1, Ordering::Relaxed);
    if idx >= POOL_SIZE {
        return None;
    }
    // SAFETY: each index is handed out to exactly one caller (monotonic counter), and
    // the pool outlives every test in this single-threaded boot.
    unsafe { Some(virt_to_phys(addr_of!(POOL[idx]) as u64)) }
}

/// Bump allocator capped after `n` successful allocations, to deterministically exhaust
/// scaffold frames regardless of `POOL_SIZE`.
fn capped_alloc(n: usize) -> impl FnMut() -> Option<u64> {
    let mut remaining = n;
    move || {
        if remaining == 0 {
            return None;
        }
        remaining -= 1;
        bump_alloc_from_pool()
    }
}

fn ram_entry(start: u64, size: u64) -> UnifiedMemoryEntry {
    UnifiedMemoryEntry {
        start,
        size,
        memory_type: 7, // EfiConventionalMemory
        _pad: 0,
        attribute: 0,
        is_usable: true,
    }
}

/// Contract: a single small, unaligned region maps entirely as 4 KiB pages, and every
/// page resolves VA == PA afterward.
/// Failure Impact: the fallback path (small RAM / `use_huge_pages = false`) is the only
/// mapping strategy for platforms without huge-page support — a bug here would corrupt
/// or omit mappings for the entire fallback boot path.
#[test_case]
fn test_single_small_region_4k_only() {
    // SAFETY: single-threaded test context; POOL is reset before use.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(0x0010_0000, 3 * PAGE_SIZE_U64);

        let stats =
            build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        assert_eq!(stats.small_4k_pages, 3);
        assert_eq!(stats.huge_2m_pages, 0);
        assert_eq!(stats.regions_mapped, 1);

        for i in 0..3 {
            let va = 0x0010_0000 + i * PAGE_SIZE_U64;
            assert_eq!(resolve_phys_via_root(pml4, va), Some((va, PAGE_SIZE_U64)));
        }
        assert!(validate_direct_map_coverage(pml4, [region].iter(), is_phase1_ram).is_ok());
    }
}

/// Contract: a region that is exactly one 2 MiB-aligned huge page uses exactly one PD
/// huge leaf and allocates no PT frame at all.
/// Failure Impact: this is the bulk path for all of real RAM on a large-memory
/// machine — a bug here means either a huge-page-count regression (defeats the whole
/// point of Phase 1a) or a coverage gap on real hardware.
#[test_case]
fn test_region_exactly_2mib_aligned_uses_one_huge_page() {
    // SAFETY: see test_single_small_region_4k_only.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(HUGE_PAGE_SIZE_2M, HUGE_PAGE_SIZE_2M);

        let stats =
            build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, true).unwrap();
        assert_eq!(stats.huge_2m_pages, 1);
        assert_eq!(stats.small_4k_pages, 0);
        assert_eq!(stats.pt_frames_allocated, 0);

        assert_eq!(
            resolve_phys_via_root(pml4, HUGE_PAGE_SIZE_2M + 0x1234),
            Some((HUGE_PAGE_SIZE_2M + 0x1234, HUGE_PAGE_SIZE_2M))
        );
        assert!(validate_direct_map_coverage(pml4, [region].iter(), is_phase1_ram).is_ok());
    }
}

/// Contract: `use_huge_pages = false` forces 4 KiB pages even for a perfectly
/// 2-MiB-aligned region — the required fallback for platforms/first-cut builds that
/// skip huge-page support.
/// Failure Impact: without this flag being honored, there would be no way to bisect a
/// Phase 1 coverage bug from a Phase 1a huge-page-specific bug.
#[test_case]
fn test_use_huge_pages_false_forces_4k_even_when_aligned() {
    // SAFETY: see test_single_small_region_4k_only.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(HUGE_PAGE_SIZE_2M, HUGE_PAGE_SIZE_2M);

        let stats =
            build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        assert_eq!(stats.huge_2m_pages, 0);
        assert_eq!(stats.small_4k_pages, HUGE_PAGE_SIZE_2M / PAGE_SIZE_U64);
    }
}

/// Contract: unaligned head/tail bytes around one full 2 MiB huge page fall back to
/// 4 KiB pages, while the fully-aligned middle chunk still uses one huge page.
/// Failure Impact: real memory maps are never neatly 2-MiB-aligned end to end — a bug
/// in this boundary logic would either skip bytes (coverage gap) or overlap/corrupt
/// entries at the huge/4k boundary.
#[test_case]
fn test_unaligned_head_and_tail_falls_back_to_4k() {
    // Region: [2M - 4K, 2M - 4K + 4K + 2M + 4K) = one page before the 2M boundary,
    // one full 2M-aligned huge page, one page after.
    let start = HUGE_PAGE_SIZE_2M - PAGE_SIZE_U64;
    let size = PAGE_SIZE_U64 + HUGE_PAGE_SIZE_2M + PAGE_SIZE_U64;

    // SAFETY: see test_single_small_region_4k_only.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(start, size);

        let stats =
            build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, true).unwrap();
        assert_eq!(stats.huge_2m_pages, 1);
        assert_eq!(stats.small_4k_pages, 2);
        assert!(validate_direct_map_coverage(pml4, [region].iter(), is_phase1_ram).is_ok());
    }
}

/// Contract: a region the classifier rejects (not usable, not ACPI-reclaimable) is
/// left entirely unmapped.
/// Failure Impact: mapping firmware-reserved/unusable regions as if they were RAM
/// would let the kernel treat non-RAM (e.g. `EfiUnusableMemory`) as allocatable.
#[test_case]
fn test_non_ram_region_is_skipped() {
    // SAFETY: see test_single_small_region_4k_only.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = UnifiedMemoryEntry {
            start: 0x0020_0000,
            size: PAGE_SIZE_U64,
            memory_type: 0, // EfiReservedMemoryType
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };

        let stats =
            build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        assert_eq!(stats.regions_mapped, 0);
        assert_eq!(stats.regions_considered, 1);
        assert_eq!(resolve_phys_via_root(pml4, 0x0020_0000), None);
    }
}

/// Contract: `is_phase1_ram` maps ACPI-reclaimable memory (EFI type 9) even though it
/// is not marked `is_usable`, per the design doc's §4 type table.
/// Failure Impact: without this, a future ACPI table parser reading reclaim memory
/// before RAM is reclaimed would depend on firmware coverage again — reintroducing P2.
#[test_case]
fn test_acpi_reclaim_region_is_mapped_by_default_classifier() {
    // SAFETY: see test_single_small_region_4k_only.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = UnifiedMemoryEntry {
            start: 0x0030_0000,
            size: PAGE_SIZE_U64,
            memory_type: 9, // EfiACPIReclaimMemory
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };

        let stats =
            build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        assert_eq!(stats.regions_mapped, 1);
        assert_eq!(
            resolve_phys_via_root(pml4, 0x0030_0000),
            Some((0x0030_0000, PAGE_SIZE_U64))
        );
    }
}

/// Contract: exhausting the scaffold-frame allocator returns `Err`, not a panic — the
/// builder is a pure function and leaves fatal-vs-recoverable decisions to its caller.
/// Failure Impact: a panic here (instead of a catchable `Err`) would make it impossible
/// for the boot-time call site to log context before deciding how to fail.
#[test_case]
fn test_out_of_scaffold_frames_returns_err_not_panic() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        // A single 4 KiB page on a fresh tree needs 3 scaffold frames (PDPT+PD+PT).
        // Capping at 2 forces the 3rd (PT) allocation to fail.
        let mut alloc = capped_alloc(2);
        let region = ram_entry(0x0040_0000, PAGE_SIZE_U64);

        let result = build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false);
        assert_eq!(result, Err(DirectMapError::OutOfScaffoldFrames));
    }
}

/// Contract: mapping the same page twice with the same (identity) target succeeds
/// idempotently instead of erroring.
/// Failure Impact: real memory maps can list adjoining/duplicate descriptors; treating
/// a second identical pass as an error would make the builder unusable for Phase 2's
/// "reuse the same table, wider classifier" call pattern.
#[test_case]
fn test_overlapping_identical_regions_is_idempotent() {
    // SAFETY: see test_single_small_region_4k_only.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(0x0050_0000, PAGE_SIZE_U64);

        let result = build_direct_map(
            pml4,
            [region, region].iter(),
            is_phase1_ram,
            &mut alloc,
            false,
        );
        assert!(result.is_ok());
        assert_eq!(
            resolve_phys_via_root(pml4, 0x0050_0000),
            Some((0x0050_0000, PAGE_SIZE_U64))
        );
    }
}

/// Contract: reusing the same table across two `build_direct_map` calls (as Phase 1
/// then Phase 2 will do) rejects a huge-page request that collides with an
/// already-installed 4 KiB sub-table, instead of silently corrupting the tree.
/// Failure Impact: an undetected collision would overwrite a live PD entry that still
/// has a present PT hanging off it in the encoded frame bits, orphaning that PT and
/// corrupting every mapping it held.
#[test_case]
fn test_second_build_call_with_huge_page_collision_is_rejected() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        // First pass: map one 4 KiB page inside what will become a 2 MiB huge window.
        let small_region = ram_entry(2 * HUGE_PAGE_SIZE_2M, PAGE_SIZE_U64);
        build_direct_map(
            pml4,
            [small_region].iter(),
            is_phase1_ram,
            &mut alloc,
            false,
        )
        .unwrap();

        // Second pass, same table: a huge-aligned region covering the same address.
        let huge_region = ram_entry(2 * HUGE_PAGE_SIZE_2M, HUGE_PAGE_SIZE_2M);
        let result = build_direct_map(pml4, [huge_region].iter(), is_phase1_ram, &mut alloc, true);
        assert_eq!(
            result,
            Err(DirectMapError::HugePageCollision {
                va: 2 * HUGE_PAGE_SIZE_2M
            })
        );
    }
}

/// Contract: `validate_direct_map_coverage` catches a real gap in an otherwise-built
/// table (simulating a builder bug), rather than silently passing.
/// Failure Impact: this is the exact safety net the design doc calls out — without it,
/// a Phase 1 coverage bug would surface much later as a misleading real-hardware SMM
/// reset once a future phase switches CR3 to this table.
#[test_case]
fn test_validate_direct_map_coverage_detects_synthetic_gap() {
    // SAFETY: single-threaded test context. Deliberately builds a fresh tree so the
    // scaffold-frame allocation order is deterministic: POOL[1]=PDPT, POOL[2]=PD,
    // POOL[3]=PT for the one page mapped below.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let va = 0x0060_0000_u64;
        let region = ram_entry(va, PAGE_SIZE_U64);

        build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        assert!(validate_direct_map_coverage(pml4, [region].iter(), is_phase1_ram).is_ok());

        // Corrupt the known PT frame (POOL[3]) to simulate a coverage gap.
        (*addr_of_mut!(POOL[3])).entries[pt_index(va)].clear();

        assert_eq!(
            validate_direct_map_coverage(pml4, [region].iter(), is_phase1_ram),
            Err(CoverageGap::Unmapped { va })
        );
    }
}

// ============================================================================
// Phase 2 classifier tests (`is_phase2_platform`) — part of #63, Phase 2.
// ============================================================================

/// Contract: `is_phase2_platform` accepts any region with the `EFI_MEMORY_RUNTIME`
/// attribute bit set, regardless of its memory type.
/// Failure Impact: missing a runtime-flagged region would drop a mapping the platform
/// may depend on for `SetVirtualAddressMap`/SMM, once a later phase relies on this
/// classifier instead of inherited firmware coverage.
#[test_case]
fn test_is_phase2_platform_matches_runtime_attribute_bit() {
    const EFI_MEMORY_RUNTIME: u64 = 0x8000_0000_0000_0000;
    let entry = UnifiedMemoryEntry {
        start: 0,
        size: PAGE_SIZE_U64,
        memory_type: 7, // EfiConventionalMemory - not in the type list either
        _pad: 0,
        attribute: EFI_MEMORY_RUNTIME,
        is_usable: true,
    };
    assert!(is_phase2_platform(&entry));
}

/// Contract: `is_phase2_platform` accepts exactly the EFI types the design doc's §4
/// table marks "map" for firmware/platform reasons (0, 5, 6, 10, 11, 13), and rejects
/// types outside that set when the runtime attribute bit is also clear.
/// Failure Impact: an overly narrow classifier would leave e.g. ACPI NVS or MMIO
/// unmapped once the kernel table becomes load-bearing; an overly wide one would map
/// loader-transient memory (`BootServicesCode`/`Data`) the design doc says to drop.
#[test_case]
fn test_is_phase2_platform_matches_known_types_only() {
    for &memory_type in &[0u32, 5, 6, 10, 13] {
        let entry = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };
        assert!(
            is_phase2_platform(&entry),
            "memory_type {} should be classified as platform",
            memory_type
        );
    }

    // 11 (MemoryMappedIO) and 12 (MemoryMappedIOPortSpace) are deliberately excluded
    // here - see is_mmio/test_is_mmio_matches_type_11_and_12_only below.
    for &memory_type in &[1u32, 3, 4, 7, 9, 11, 12] {
        let entry = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: 0,
            is_usable: memory_type == 7,
        };
        assert!(
            !is_phase2_platform(&entry),
            "memory_type {} should NOT be classified as platform",
            memory_type
        );
    }
}

/// Contract: a Phase 1 (RAM) pass and a Phase 2 (platform) pass over disjoint address
/// ranges can reuse the same table, each producing a fully valid, independently
/// checkable coverage — the exact "build once per classifier, same PML4" pattern
/// `vmm::direct_map::build_full_kernel_pml4` uses in production.
/// Failure Impact: if the two passes corrupted each other's mappings (e.g. via a stale
/// PD/PDPT reused incorrectly), a real boot would either lose RAM coverage or firmware/
/// MMIO coverage depending on call order — exactly the class of bug the pre-CR3-switch
/// coverage validation exists to catch before a CR3 switch ever happens.
#[test_case]
fn test_phase1_and_phase2_passes_coexist_on_the_same_table() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        let ram_region = ram_entry(0x0070_0000, PAGE_SIZE_U64);
        let platform_region = UnifiedMemoryEntry {
            start: 0x0080_0000,
            size: PAGE_SIZE_U64,
            memory_type: 0, // EfiReservedMemoryType
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };

        build_direct_map(pml4, [ram_region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        build_direct_map(
            pml4,
            [platform_region].iter(),
            is_phase2_platform,
            &mut alloc,
            false,
        )
        .unwrap();

        assert!(validate_direct_map_coverage(pml4, [ram_region].iter(), is_phase1_ram).is_ok());
        assert!(
            validate_direct_map_coverage(pml4, [platform_region].iter(), is_phase2_platform)
                .is_ok()
        );
        assert_eq!(
            resolve_phys_via_root(pml4, 0x0070_0000),
            Some((0x0070_0000, PAGE_SIZE_U64))
        );
        assert_eq!(
            resolve_phys_via_root(pml4, 0x0080_0000),
            Some((0x0080_0000, PAGE_SIZE_U64))
        );
    }
}

// ============================================================================
// Phase 3: `map_wc_range` (explicit GOP-framebuffer-style write-combining mapping) -
// part of #63, Phase 3.
// ============================================================================

/// Contract: `map_wc_range` maps every page of `[base, base+size)` identity (VA == PA),
/// rounding to page boundaries, with the write-combining PTE flags (PWT set, PCD
/// clear) and NX — the exact combination `main.rs`'s PAT1 setup expects.
/// Failure Impact: a wrong flag combination would either lose write-combining
/// performance (falls back to a slower cache mode) or, worse, allow code execution
/// from framebuffer memory if NX were missing — the entire point of Phase 3 mapping
/// this range explicitly instead of inheriting whatever the firmware happened to set.
#[test_case]
fn test_map_wc_range_sets_identity_mapping_and_wc_nx_flags() {
    // SAFETY: single-threaded test context; POOL is reset before use. Fresh pool means
    // deterministic allocation order: POOL[1]=PDPT, POOL[2]=PD, POOL[3]=PT.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        let base = 0x0090_0000_u64; // not page-boundary-sensitive; already 4 KiB aligned
        let size = 2 * PAGE_SIZE_U64;
        map_wc_range(pml4, base, size, &mut alloc).unwrap();

        assert_eq!(
            resolve_phys_via_root(pml4, base),
            Some((base, PAGE_SIZE_U64))
        );
        assert_eq!(
            resolve_phys_via_root(pml4, base + PAGE_SIZE_U64),
            Some((base + PAGE_SIZE_U64, PAGE_SIZE_U64))
        );

        let pt = &*addr_of!(POOL[3]);
        let entry = pt.entries[pt_index(base)];
        assert!(entry.present());
        assert!(entry.writable());
        assert!(!entry.huge());
        assert!(entry.no_execute(), "framebuffer mapping must be NX");
        assert_ne!(
            entry.raw() & ENTRY_PWT,
            0,
            "PWT must be set for write-combining"
        );
        assert_eq!(
            entry.raw() & ENTRY_PCD,
            0,
            "PCD must be clear for write-combining"
        );
    }
}

/// Contract: `map_wc_range` rounds a non-page-aligned `base`/`size` outward so the
/// entire requested byte range ends up covered, not truncated.
/// Failure Impact: a real GOP framebuffer's `base_address` is not guaranteed page
/// aligned by firmware; truncating the range would leave the tail of the framebuffer
/// unmapped once this table becomes load-bearing.
#[test_case]
fn test_map_wc_range_rounds_unaligned_base_and_size_outward() {
    // SAFETY: see test_map_wc_range_sets_identity_mapping_and_wc_nx_flags.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        let base = 0x00A0_0123_u64; // deliberately unaligned
        let size = PAGE_SIZE_U64 + 1; // spills one byte into a second page
        map_wc_range(pml4, base, size, &mut alloc).unwrap();

        let rounded_start = base & !(PAGE_SIZE_U64 - 1);
        assert_eq!(
            resolve_phys_via_root(pml4, rounded_start),
            Some((rounded_start, PAGE_SIZE_U64))
        );
        assert_eq!(
            resolve_phys_via_root(pml4, rounded_start + PAGE_SIZE_U64),
            Some((rounded_start + PAGE_SIZE_U64, PAGE_SIZE_U64))
        );
    }
}

// ============================================================================
// Regression: `build_direct_map` must round each region to page boundaries before
// mapping (found via `direct_map_full_switch_test.rs`'s real-QEMU-memory-map run).
// ============================================================================

/// Contract: two adjacent regions that are individually page-unaligned but share a
/// containing page (e.g. classic QEMU/SeaBIOS `[0x0, 0x9FC00)` usable RAM immediately
/// followed by a `[0x9FC00, 0xA0000)` reserved region — both fall inside the same 4 KiB
/// page `[0x9F000, 0xA0000)`) build successfully instead of spuriously erroring.
/// Failure Impact: without rounding each region to page boundaries first, the second
/// region's unaligned start gets silently truncated by `phys_to_pfn`'s `>>12` when
/// written into a PTE, which then no longer matches the *unrounded* address used for
/// the idempotency check against the page Phase 1 already mapped — a real page,
/// already correctly identity-mapped, gets misreported as a conflicting `Overlap`.
/// This exact map shape is what `direct_map_full_switch_test.rs` hit against QEMU's
/// real E820 map before this fix.
#[test_case]
fn test_adjacent_page_unaligned_regions_sharing_a_page_do_not_spuriously_overlap() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        let usable = ram_entry(0x0, 0x9FC00);
        let reserved = UnifiedMemoryEntry {
            start: 0x9FC00,
            size: 0x400,    // ends at 0xA0000
            memory_type: 0, // EfiReservedMemoryType
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };

        build_direct_map(pml4, [usable].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        build_direct_map(
            pml4,
            [reserved].iter(),
            is_phase2_platform,
            &mut alloc,
            false,
        )
        .unwrap();

        // The shared page must resolve as plain identity (VA == PA) throughout.
        assert_eq!(
            resolve_phys_via_root(pml4, 0x9F000),
            Some((0x9F000, PAGE_SIZE_U64))
        );
        assert_eq!(
            resolve_phys_via_root(pml4, 0x9FC00),
            Some((0x9FC00, PAGE_SIZE_U64))
        );
    }
}

// ============================================================================
// Phase 5 (#63): the whole identity/direct map (RAM + platform) is NX.
// ============================================================================

/// Contract: a 2 MiB huge-page RAM mapping is created NX.
/// Failure Impact: the design doc's Phase 5 goal ("data + direct map: NX") would be
/// unmet for the huge-page bulk path — the vast majority of real RAM — leaving code
/// injected into RAM through the identity map executable.
#[test_case]
fn test_2mib_ram_mapping_is_nx() {
    // SAFETY: single-threaded test context; fresh pool means POOL[2] is the PD frame.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(HUGE_PAGE_SIZE_2M, HUGE_PAGE_SIZE_2M);
        build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, true).unwrap();

        let pd = &*addr_of!(POOL[2]);
        let entry = pd.entries[pd_index(HUGE_PAGE_SIZE_2M)];
        assert!(entry.present());
        assert!(entry.huge());
        assert!(entry.no_execute(), "2 MiB RAM mapping must be NX");
    }
}

/// Contract: a 4 KiB RAM mapping and a 4 KiB Phase 2 platform mapping are both created
/// NX.
/// Failure Impact: same as above, but for the 4 KiB fallback/edge path and for
/// firmware/platform regions (MMIO in particular — an executable MMIO mapping is a
/// classic privilege-escalation primitive).
#[test_case]
fn test_4kib_ram_and_platform_mappings_are_nx() {
    // SAFETY: single-threaded test context; fresh pool means POOL[3] is the PT frame.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let region = ram_entry(0x0060_0000, PAGE_SIZE_U64);
        build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();

        let pt = &*addr_of!(POOL[3]);
        let entry = pt.entries[pt_index(0x0060_0000)];
        assert!(entry.present());
        assert!(!entry.huge());
        assert!(entry.no_execute(), "4 KiB RAM mapping must be NX");

        let platform = UnifiedMemoryEntry {
            start: 0x0061_0000,
            size: PAGE_SIZE_U64,
            memory_type: 0, // EfiReservedMemoryType
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };
        build_direct_map(
            pml4,
            [platform].iter(),
            is_phase2_platform,
            &mut alloc,
            false,
        )
        .unwrap();
        let platform_entry = pt.entries[pt_index(0x0061_0000)];
        assert!(platform_entry.present());
        assert!(platform_entry.no_execute(), "platform mapping must be NX");
    }
}

// ============================================================================
// Follow-up (review #63, point 5): MMIO is mapped uncacheable, not write-back.
// ============================================================================

/// Contract: `is_mmio` accepts exactly `EfiMemoryMappedIO` (11) and
/// `EfiMemoryMappedIOPortSpace` (12), and rejects everything `is_phase2_platform`
/// accepts.
/// Failure Impact: if `is_mmio` and `is_phase2_platform` overlapped, a region could be
/// mapped twice with conflicting caching attributes (WB from one pass, UC from the
/// other) — undefined behavior on real hardware for device memory. Missing type 12
/// entirely (as originally shipped) instead silently drops those regions from every
/// classifier — no mapping at all, not just a wrong one.
#[test_case]
fn test_is_mmio_matches_type_11_and_12_only() {
    for &memory_type in &[11u32, 12] {
        let mmio = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };
        assert!(
            is_mmio(&mmio),
            "memory_type {} should be classified as MMIO",
            memory_type
        );
        assert!(
            !is_phase2_platform(&mmio),
            "memory_type {} must not also be accepted by is_phase2_platform",
            memory_type
        );
    }

    for &memory_type in &[0u32, 1, 5, 6, 7, 9, 10, 13] {
        let entry = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: 0,
            is_usable: memory_type == 7,
        };
        assert!(
            !is_mmio(&entry),
            "memory_type {} should NOT be classified as MMIO",
            memory_type
        );
    }
}

/// Regression (#63 review point 5): a `MemoryMappedIO` (11) or `MemoryMappedIOPortSpace`
/// (12) region that ALSO carries the `EFI_MEMORY_RUNTIME` attribute must still be
/// rejected by `is_phase2_platform` (and accepted by `is_mmio`). The plain
/// `test_is_mmio_matches_type_11_and_12_only` above only covers `attribute == 0`, so it
/// misses exactly this overlap.
/// Failure Impact: OVMF marks such a region on the UEFI path. If `is_phase2_platform`'s
/// `EFI_MEMORY_RUNTIME` clause re-claimed it, the Phase 2 pass would map the window
/// write-back — as a 2 MiB huge page when aligned — and the later uncacheable MMIO pass
/// would hit a `HugePageCollision`, panicking the kernel-owned table build in
/// `build_full_kernel_pml4` (this is the QEMU/OVMF boot hang this fix resolves).
/// Release-blocking on UEFI.
#[test_case]
fn test_mmio_with_runtime_attribute_is_not_phase2() {
    const EFI_MEMORY_RUNTIME: u64 = 0x8000_0000_0000_0000;
    for &memory_type in &[11u32, 12] {
        let mmio_runtime = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: EFI_MEMORY_RUNTIME,
            is_usable: false,
        };
        assert!(is_mmio(&mmio_runtime));
        assert!(
            !is_phase2_platform(&mmio_runtime),
            "type-{} MMIO with EFI_MEMORY_RUNTIME must be handled by the UC path only, \
             never re-claimed by is_phase2_platform",
            memory_type
        );
    }
}

/// Contract: `build_uc_direct_map` maps MMIO as present, NX, uncacheable (PCD set,
/// PWT clear), 4 KiB only — never a 2 MiB huge leaf.
/// Failure Impact: mapping device memory write-back (the default `map_2m_page`/
/// `map_4k_page` caching) risks stale reads and write reordering against real
/// hardware registers — silent, hard-to-diagnose device misbehavior, not a crash.
#[test_case]
fn test_build_uc_direct_map_sets_uncacheable_nx_4kib_only() {
    // SAFETY: single-threaded test context; fresh pool means POOL[3] is the PT frame.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;
        let mmio = UnifiedMemoryEntry {
            start: 0x0062_0000,
            size: PAGE_SIZE_U64,
            memory_type: 11,
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };

        let stats = build_uc_direct_map(pml4, [mmio].iter(), is_mmio, &mut alloc).unwrap();
        assert_eq!(stats.small_4k_pages, 1);
        assert_eq!(stats.huge_2m_pages, 0);

        let pt = &*addr_of!(POOL[3]);
        let entry = pt.entries[pt_index(0x0062_0000)];
        assert!(entry.present());
        assert!(!entry.huge());
        assert!(entry.no_execute(), "MMIO mapping must be NX");
        assert_eq!(
            entry.raw() & ENTRY_PCD,
            ENTRY_PCD,
            "PCD must be set for MMIO"
        );
        assert_eq!(entry.raw() & ENTRY_PWT, 0, "PWT must be clear for MMIO");

        assert_eq!(
            resolve_phys_via_root(pml4, 0x0062_0000),
            Some((0x0062_0000, PAGE_SIZE_U64))
        );
        assert!(validate_direct_map_coverage(pml4, [mmio].iter(), is_mmio).is_ok());
    }
}

// ============================================================================
// Follow-up to the #63 review: loader-owned regions and essential-address checks.
// ============================================================================

/// Contract: `is_loader_owned` accepts exactly `EfiLoaderCode` (1) and `EfiLoaderData`
/// (2), and rejects other types.
/// Failure Impact: the UEFI loader allocates the PMM-metadata region as
/// `EfiLoaderData` — missing this type here would leave that region unmapped once the
/// kernel-owned table becomes active, corrupting or faulting on the very first PMM
/// bitmap access after the switch.
#[test_case]
fn test_is_loader_owned_matches_types_1_and_2_only() {
    for &memory_type in &[1u32, 2] {
        let entry = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };
        assert!(
            is_loader_owned(&entry),
            "memory_type {} should be classified as loader-owned",
            memory_type
        );
    }

    for &memory_type in &[0u32, 3, 5, 7, 9, 11, 12] {
        let entry = UnifiedMemoryEntry {
            start: 0,
            size: PAGE_SIZE_U64,
            memory_type,
            _pad: 0,
            attribute: 0,
            is_usable: memory_type == 7,
        };
        assert!(
            !is_loader_owned(&entry),
            "memory_type {} should NOT be classified as loader-owned",
            memory_type
        );
    }
}

/// Synthetic `BootInfo` for the `validate_essential_boot_addresses` tests below —
/// its own address stands in for "the address the kernel dereferences", exactly like
/// `pmm_uefi_test.rs`'s `SYN_BOOT_INFO`.
static mut ESSENTIAL_TEST_BOOT_INFO: BootInfo = BootInfo {
    magic: 0,
    video_type: VideoModeType::VgaText,
    fb_info: FramebufferInfo {
        base_address: 0,
        size: 0,
        width: 0,
        height: 0,
        pixels_per_scanline: 0,
        pixel_format: PixelFormat::Bgr,
    },
    memory_map_addr: 0,
    memory_map_len: 0,
    kernel_size: 0,
    pmm_metadata_base: 0,
    pmm_metadata_size: 0,
    boot_year: 0,
    boot_month: 0,
    boot_day: 0,
    boot_hour: 0,
    boot_minute: 0,
    boot_second: 0,
    boot_timezone: 0,
};

/// Contract: `validate_essential_boot_addresses` succeeds when the `BootInfo`
/// structure's own address is actually covered by the table.
/// Failure Impact: a false negative here (reporting a gap that isn't real) would make
/// the check useless — every real boot would panic on it.
#[test_case]
fn test_validate_essential_boot_addresses_passes_when_covered() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        // Dereference the PHYSICAL-equivalent address, exactly like production does
        // (`&*(boot_info_raw as *const BootInfo)` in `switch_to_direct_map`) —
        // `boot_info as *const _ as u64` inside
        // `validate_essential_boot_addresses` must match the address the `ram_entry`
        // region below covers, not the kernel-image (higher-half) static address.
        let boot_info_phys = virt_to_phys(addr_of!(ESSENTIAL_TEST_BOOT_INFO) as u64);
        let region = ram_entry(
            boot_info_phys & !(PAGE_SIZE_U64 - 1),
            PAGE_SIZE_U64 * 2, // generous: covers the struct regardless of page straddling
        );
        build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, false).unwrap();

        let boot_info_ref = &*(boot_info_phys as *const BootInfo);
        assert!(validate_essential_boot_addresses(pml4, boot_info_ref).is_ok());
    }
}

/// Contract: `validate_essential_boot_addresses` reports a gap when the `BootInfo`
/// structure itself is NOT covered by any mapping — this is the exact class of bug the
/// #63 review found (EfiLoaderData/EfiLoaderCode regions unmapped by either
/// classifier), pinned independently of whatever classifiers exist today.
/// Failure Impact: without this check, a future classifier regression that stops
/// covering the `BootInfo`/memory-map/PMM-metadata addresses would only surface as an
/// unexplained fault or silent corruption deep in `heap::init`, far from the root cause.
#[test_case]
fn test_validate_essential_boot_addresses_detects_unmapped_bootinfo() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        // Map some unrelated RAM, but deliberately NOT the BootInfo's own address.
        let unrelated_region = ram_entry(0x0070_0000, PAGE_SIZE_U64);
        build_direct_map(
            pml4,
            [unrelated_region].iter(),
            is_phase1_ram,
            &mut alloc,
            false,
        )
        .unwrap();

        let boot_info_phys = virt_to_phys(addr_of!(ESSENTIAL_TEST_BOOT_INFO) as u64);
        let boot_info_ref = &*(boot_info_phys as *const BootInfo);
        // Unlike `build_direct_map`, `validate_byte_range_coverage` does not floor its
        // start address to a page boundary before the first `resolve_phys_via_root`
        // call, so the reported gap is at the exact (unfloored) BootInfo address.
        assert_eq!(
            validate_essential_boot_addresses(pml4, boot_info_ref),
            Err(CoverageGap::Unmapped { va: boot_info_phys })
        );
    }
}

// ============================================================================
// Regression (review #63, point 3): a UEFI-shaped memory layout, where the PMM
// metadata region lives in EfiLoaderData (as kaosldr_uefi actually allocates it),
// must build successfully through `build_full_kernel_pml4` — the same function
// `switch_to_direct_map` uses for the real CR3 switch. Exercises the *build* only
// (no `write_cr3`), so an incomplete synthetic map cannot crash the test kernel, while
// still directly covering the gap `direct_map_full_switch_test.rs` (which only ever
// boots via the BIOS loader, where metadata falls back to plain usable RAM) does not.
// ============================================================================

#[repr(C, align(4096))]
struct PageAlignedBuf([u8; PAGE_SIZE_U64 as usize]);
static mut UEFI_METADATA_BUF: PageAlignedBuf = PageAlignedBuf([0u8; PAGE_SIZE_U64 as usize]);

static mut UEFI_LAYOUT_REGIONS: [UnifiedMemoryEntry; 2] = [
    UnifiedMemoryEntry {
        start: 0x0010_0000,
        size: 0x0100_0000, // 16 MiB of ordinary usable RAM
        memory_type: 7,    // EfiConventionalMemory
        _pad: 0,
        attribute: 0,
        is_usable: true,
    },
    UnifiedMemoryEntry {
        start: 0, // filled in at test time with UEFI_METADATA_BUF's physical address
        size: PAGE_SIZE_U64,
        memory_type: 2, // EfiLoaderData - matches kaosldr_uefi's allocate_pages(0, 2, ...)
        _pad: 0,
        attribute: 0,
        is_usable: false,
    },
];

static mut UEFI_LAYOUT_BOOT_INFO: BootInfo = BootInfo {
    magic: 0,
    video_type: VideoModeType::VgaText,
    fb_info: FramebufferInfo {
        base_address: 0,
        size: 0,
        width: 0,
        height: 0,
        pixels_per_scanline: 0,
        pixel_format: PixelFormat::Bgr,
    },
    memory_map_addr: 0,
    memory_map_len: 2,
    kernel_size: 0,
    pmm_metadata_base: 0,
    pmm_metadata_size: PAGE_SIZE_U64,
    boot_year: 0,
    boot_month: 0,
    boot_day: 0,
    boot_hour: 0,
    boot_minute: 0,
    boot_second: 0,
    boot_timezone: 0,
};

/// Contract: `build_full_kernel_pml4` succeeds and correctly maps the PMM-metadata
/// region when it lives in `EfiLoaderData` — the real UEFI placement — instead of
/// panicking or leaving it uncovered.
/// Failure Impact: without the `is_loader_owned` classifier and the
/// `validate_essential_boot_addresses` check, this exact layout would previously have
/// left `pmm_metadata_base` unmapped; the first PMM bitmap access after a real
/// `switch_to_direct_map` would then fault or (via on-demand paging) silently swap in
/// a fresh zeroed page over the real metadata, corrupting the allocator.
#[test_case]
fn test_build_full_kernel_pml4_maps_uefi_style_loader_data_metadata() {
    // SAFETY: single-threaded test context. `old_pml4_phys` is the currently active
    // (real) PML4 - only its slot 256 entry is read, to seed the higher-half mirror;
    // no write happens to it and CR3 is never switched.
    unsafe {
        let metadata_phys = virt_to_phys(addr_of!(UEFI_METADATA_BUF) as u64);
        UEFI_LAYOUT_REGIONS[1].start = metadata_phys;
        UEFI_LAYOUT_BOOT_INFO.memory_map_addr = virt_to_phys(addr_of!(UEFI_LAYOUT_REGIONS) as u64);
        UEFI_LAYOUT_BOOT_INFO.pmm_metadata_base = metadata_phys;

        let boot_info_phys = virt_to_phys(addr_of!(UEFI_LAYOUT_BOOT_INFO) as u64);
        let boot_info_ref = &*(boot_info_phys as *const BootInfo);
        let regions_ref =
            &*(UEFI_LAYOUT_BOOT_INFO.memory_map_addr as *const [UnifiedMemoryEntry; 2]);

        let old_pml4 = page_table::read_cr3() & 0x000F_FFFF_FFFF_F000;
        let mut alloc = page_table::alloc_frame_phys;

        let (new_pml4, _ram_stats, _platform_stats, loader_stats, _mmio_stats) =
            build_full_kernel_pml4(old_pml4, regions_ref, boot_info_ref, &mut alloc).unwrap();

        assert_eq!(loader_stats.regions_mapped, 1);
        assert_eq!(
            resolve_phys_via_root(new_pml4, metadata_phys),
            Some((metadata_phys, PAGE_SIZE_U64))
        );
        assert!(validate_essential_boot_addresses(new_pml4, boot_info_ref).is_ok());
    }
}

// ============================================================================
// Regression (review #63, point 4): the GOP framebuffer must actually be mapped by
// `build_full_kernel_pml4` when one is present, not just documented as "the caller's
// responsibility" while no caller ever does it.
// ============================================================================

#[repr(C, align(4096))]
struct FbBuf([u8; 3 * PAGE_SIZE_U64 as usize]);
static mut FB_TEST_BUF: FbBuf = FbBuf([0u8; 3 * PAGE_SIZE_U64 as usize]);

static mut FB_TEST_REGIONS: [UnifiedMemoryEntry; 2] = [
    UnifiedMemoryEntry {
        start: 0, // filled in at test time: BootInfo's own page
        size: PAGE_SIZE_U64 * 2,
        memory_type: 7,
        _pad: 0,
        attribute: 0,
        is_usable: true,
    },
    UnifiedMemoryEntry {
        start: 0, // filled in at test time: the regions array's own page
        size: PAGE_SIZE_U64 * 2,
        memory_type: 7,
        _pad: 0,
        attribute: 0,
        is_usable: true,
    },
];

static mut FB_TEST_BOOT_INFO: BootInfo = BootInfo {
    magic: 0,
    video_type: VideoModeType::Framebuffer,
    fb_info: FramebufferInfo {
        base_address: 0, // filled in at test time
        size: 0,         // filled in at test time
        width: 0,
        height: 0,
        pixels_per_scanline: 0,
        pixel_format: PixelFormat::Bgr,
    },
    memory_map_addr: 0,
    memory_map_len: 2,
    kernel_size: 0,
    pmm_metadata_base: 0,
    pmm_metadata_size: 0,
    boot_year: 0,
    boot_month: 0,
    boot_day: 0,
    boot_hour: 0,
    boot_minute: 0,
    boot_second: 0,
    boot_timezone: 0,
};

/// Contract: when `BootInfo.video_type == Framebuffer` and `fb_info.base_address != 0`,
/// `build_full_kernel_pml4` maps the whole `[base_address, base_address + size)` range,
/// not just the classifier-covered RAM/platform/loader regions.
///
/// The complementary "no framebuffer -> not mapped, no panic" case is already covered
/// implicitly by `test_build_full_kernel_pml4_maps_uefi_style_loader_data_metadata`
/// (that test's `BootInfo` stays `VideoModeType::VgaText` with `base_address == 0` and
/// completes without ever calling `map_wc_range`).
///
/// Failure Impact: before this fix, `map_wc_range` was implemented and unit-tested in
/// isolation but never actually called from the only production call site
/// (`switch_to_direct_map`) — a real UEFI/GOP boot would lose the framebuffer mapping
/// entirely once CR3 is switched to the kernel-owned table.
#[test_case]
fn test_build_full_kernel_pml4_maps_framebuffer_when_present() {
    // SAFETY: single-threaded test context. `old_pml4_phys` is only read (slot 256),
    // never written; CR3 is never switched.
    unsafe {
        let boot_info_phys = virt_to_phys(addr_of!(FB_TEST_BOOT_INFO) as u64);
        let regions_phys = virt_to_phys(addr_of!(FB_TEST_REGIONS) as u64);
        let fb_phys = virt_to_phys(addr_of!(FB_TEST_BUF) as u64);

        FB_TEST_REGIONS[0].start = boot_info_phys & !(PAGE_SIZE_U64 - 1);
        FB_TEST_REGIONS[1].start = regions_phys & !(PAGE_SIZE_U64 - 1);
        FB_TEST_BOOT_INFO.memory_map_addr = regions_phys;
        FB_TEST_BOOT_INFO.fb_info.base_address = fb_phys;
        FB_TEST_BOOT_INFO.fb_info.size = 3 * PAGE_SIZE_U64 as usize;

        let boot_info_ref = &*(boot_info_phys as *const BootInfo);
        let regions_ref = &*(regions_phys as *const [UnifiedMemoryEntry; 2]);
        let old_pml4 = page_table::read_cr3() & 0x000F_FFFF_FFFF_F000;
        let mut alloc = page_table::alloc_frame_phys;

        let (new_pml4, ..) =
            build_full_kernel_pml4(old_pml4, regions_ref, boot_info_ref, &mut alloc).unwrap();

        assert_eq!(
            resolve_phys_via_root(new_pml4, fb_phys),
            Some((fb_phys, PAGE_SIZE_U64))
        );
        assert_eq!(
            resolve_phys_via_root(new_pml4, fb_phys + 2 * PAGE_SIZE_U64),
            Some((fb_phys + 2 * PAGE_SIZE_U64, PAGE_SIZE_U64))
        );
    }
}
