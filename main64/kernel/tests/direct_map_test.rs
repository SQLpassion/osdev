//! `direct_map::build_direct_map` / `validate_direct_map_coverage` / `free_direct_map_tables`
//! — pure-builder integration tests (part of #63, Phase 1).
//!
//! Most tests here build a synthetic PML4 over a small static pool of page-aligned
//! `PageTable`s, standing in for scaffold frames a real boot would draw from the PMM
//! (same trick as `pmm_uefi_test.rs`'s `META_BUF`: the buffer's own address is used as
//! the "physical" address, converted through `virt_to_phys` where it is written into a
//! page-table entry's frame field — see `page_table_test.rs` for why that conversion
//! is required). These tests never call `pmm::init`/`vmm::init` and never touch the
//! real PMM.
//!
//! The one exception is `test_free_direct_map_tables_returns_all_frames_to_pmm`, which
//! exercises the full real-PMM lifecycle (`pmm::init` + `page_table::alloc_frame_phys`)
//! since `free_direct_map_tables` releases frames back to the *real* global PMM and
//! must not be pointed at synthetic addresses that were never allocated from it.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut};
use core::sync::atomic::{AtomicUsize, Ordering};

use kaos_kernel::arch::constants::PAGE_SIZE_U64;
use kaos_kernel::boot_info::UnifiedMemoryEntry;
use kaos_kernel::memory::pmm::{self, types::virt_to_phys};
use kaos_kernel::memory::vmm::direct_map::{
    build_direct_map, free_direct_map_tables, is_phase1_ram, is_phase2_platform, map_wc_range,
    validate_direct_map_coverage, CoverageGap, DirectMapError,
};
use kaos_kernel::memory::vmm::page_table::{
    self, pt_index, resolve_phys_via_root, PageTable, ENTRY_PCD, ENTRY_PWT, HUGE_PAGE_SIZE_2M,
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

/// Contract: `free_direct_map_tables` returns every scaffold frame it allocated back to
/// the real PMM, so the Phase 1 boot-time canary (build + validate + free, no CR3
/// switch yet) has no lasting memory cost.
/// Failure Impact: a leak here would shrink available RAM by the whole direct-map
/// scaffold size (~hundreds of KiB on real hardware) on every boot, silently.
#[test_case]
fn test_free_direct_map_tables_returns_all_frames_to_pmm() {
    let free_before = pmm::with_pmm(|mgr| mgr.total_free_frames());

    let pml4 = page_table::alloc_frame_phys_or_panic("test: OOM allocating PML4");
    page_table::zero_phys_page(pml4);

    let region = ram_entry(HUGE_PAGE_SIZE_2M, HUGE_PAGE_SIZE_2M + PAGE_SIZE_U64);
    let mut alloc = page_table::alloc_frame_phys;

    // SAFETY: pml4 is a freshly allocated, zeroed frame; alloc_frame_phys draws real
    // PMM frames, all reachable via the active identity map (no CR3 switch involved).
    let stats = unsafe { build_direct_map(pml4, [region].iter(), is_phase1_ram, &mut alloc, true) }
        .unwrap();
    assert!(
        stats.pdpt_frames_allocated + stats.pd_frames_allocated + stats.pt_frames_allocated > 0
    );

    let free_after_build = pmm::with_pmm(|mgr| mgr.total_free_frames());
    // The PML4 itself plus every scaffold frame the builder drew must have been spent.
    assert!(free_after_build < free_before);

    // SAFETY: this table is not the active CR3 and nothing else references it.
    unsafe { free_direct_map_tables(pml4) };

    let free_after_release = pmm::with_pmm(|mgr| mgr.total_free_frames());
    assert_eq!(free_after_release, free_before);
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
    for &memory_type in &[0u32, 5, 6, 10, 11, 13] {
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

    for &memory_type in &[1u32, 3, 4, 7, 9] {
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
/// `vmm::direct_map::run_boot_canary` uses in production.
/// Failure Impact: if the two passes corrupted each other's mappings (e.g. via a stale
/// PD/PDPT reused incorrectly), a real boot would either lose RAM coverage or firmware/
/// MMIO coverage depending on call order — exactly the class of bug the boot-time
/// canary exists to catch before a CR3 switch ever happens.
#[test_case]
fn test_phase1_and_phase2_passes_coexist_on_the_same_table() {
    // SAFETY: single-threaded test context.
    unsafe {
        let pml4 = reset_pool();
        let mut alloc = bump_alloc_from_pool;

        let ram_region = ram_entry(0x0070_0000, PAGE_SIZE_U64);
        let mmio_region = UnifiedMemoryEntry {
            start: 0x0080_0000,
            size: PAGE_SIZE_U64,
            memory_type: 11, // EfiMemoryMappedIO
            _pad: 0,
            attribute: 0,
            is_usable: false,
        };

        build_direct_map(pml4, [ram_region].iter(), is_phase1_ram, &mut alloc, false).unwrap();
        build_direct_map(
            pml4,
            [mmio_region].iter(),
            is_phase2_platform,
            &mut alloc,
            false,
        )
        .unwrap();

        assert!(validate_direct_map_coverage(pml4, [ram_region].iter(), is_phase1_ram).is_ok());
        assert!(
            validate_direct_map_coverage(pml4, [mmio_region].iter(), is_phase2_platform).is_ok()
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
