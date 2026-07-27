//! Kernel-owned direct-map builder (Phase 1 of #63 — kernel-owned page tables on the
//! UEFI path, see `docs/todo_uefi_kernel_pagetables.md`).
//!
//! On the UEFI boot path the kernel today *inherits* the firmware's page tables:
//! `build_kernel_pml4_from_firmware` (`page_table.rs`) only clones the 512 top-level
//! PML4 entries, so every PDPT/PD/PT sub-table below them stays firmware-owned (often
//! huge pages covering all of RAM). This module builds a **kernel-owned** replacement
//! hierarchy instead — one identity-mapped (VA == PA) PML4 covering every RAM region,
//! using 2 MiB huge pages for the bulk and 4 KiB pages for unaligned edges.
//!
//! # Why this can safely allocate scaffold frames before it is active
//!
//! While this builder runs, CR3 still points at the *old*, currently-active superset
//! PML4 (today's `build_kernel_pml4_from_firmware` output). Every frame the PMM can
//! hand out is RAM the firmware itself described in its memory map — and the
//! firmware's own (huge-page) tables, still active, already cover all such RAM (that is
//! precisely problem P2 in the design doc, inverted into a guarantee here). So every
//! freshly-allocated scaffold frame (a new PDPT/PD/PT frame for the table being built)
//! is reachable through the *old*, still-active map, even while it is being populated
//! with data for the *new* map. The new map only becomes load-bearing once a future
//! phase switches CR3 to it. If this reachability assumption were ever violated (a gap
//! in the firmware's own map), the first `zero_phys_page`/write on that frame would
//! page-fault immediately, at a well-understood point — not silently corrupt later.
//!
//! This module deliberately operates on explicit physical addresses via `table_at`
//! (like `build_kernel_pml4_from_firmware` and `reserve_firmware_page_tables` already
//! do), never through the recursive-mapping API in `mapping.rs` — that API only ever
//! sees whatever PML4 is the *active* CR3, which this one is not (yet).

use core::sync::atomic::Ordering;

use crate::arch::constants::PAGE_SIZE_U64;
use crate::boot_info::{UnifiedMemoryEntry, BOOT_INFO_PTR};
use crate::memory::pmm;

use super::page_table::{
    alloc_frame_phys, alloc_frame_phys_or_panic, entry_ptr, pd_index, pdp_index, phys_to_pfn,
    pml4_index, pt_index, resolve_phys_via_root, table_at, table_entry, zero_phys_page,
    HUGE_PAGE_SIZE_2M, PT_ENTRIES,
};

/// EFI memory type 9 (`EfiACPIReclaimMemory`) — RAM that becomes usable after ACPI
/// tables are parsed. Mapped by [`is_phase1_ram`] so a future ACPI parser does not
/// depend on firmware coverage, matching the design doc's §4 type table.
const EFI_ACPI_RECLAIM_MEMORY: u32 = 9;

/// Aggregate counters returned by [`build_direct_map`]. Deliberately not a list of
/// allocated frames: `heap::init` (and therefore `Vec`) has not run yet at the point in
/// boot this builder is meant to run, so [`free_direct_map_tables`] re-walks the tree
/// structurally instead of consuming a stored list.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectMapStats {
    pub regions_considered: u32,
    pub regions_mapped: u32,
    pub huge_2m_pages: u64,
    pub small_4k_pages: u64,
    pub pdpt_frames_allocated: u64,
    pub pd_frames_allocated: u64,
    pub pt_frames_allocated: u64,
}

/// Errors [`build_direct_map`] can return. Never panics itself — the boot-time call
/// site decides whether a given error is fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectMapError {
    /// The frame allocator returned `None` while a PDPT/PD/PT scaffold frame was needed.
    OutOfScaffoldFrames,
    /// `region.start + region.size` overflowed `u64`.
    RegionOverflow { start: u64, size: u64 },
    /// A 2 MiB/4 KiB leaf was requested where the opposite granularity already
    /// occupies that slot (two input regions disagree about a shared PD window).
    HugePageCollision { va: u64 },
    /// Two regions claim the same page with different physical targets.
    Overlap {
        va: u64,
        expected_pa: u64,
        existing_pa: u64,
    },
}

/// Gap found by [`validate_direct_map_coverage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageGap {
    Unmapped {
        va: u64,
    },
    Mismatch {
        va: u64,
        expected_pa: u64,
        got_pa: u64,
    },
}

/// Default Phase 1 classifier: general-purpose usable RAM (`is_usable`) plus
/// ACPI-reclaimable memory (type 9). Phase 2 supplies a wider classifier (firmware
/// runtime/NVS/MMIO/reserved regions) to the same [`build_direct_map`] — the builder
/// itself has no opinion about which regions are RAM vs. platform-reserved.
pub fn is_phase1_ram(entry: &UnifiedMemoryEntry) -> bool {
    entry.is_usable || entry.memory_type == EFI_ACPI_RECLAIM_MEMORY
}

/// `EFI_MEMORY_RUNTIME` attribute bit (bit 63 of an EFI memory descriptor's
/// `attribute` field): the region must stay mapped for `SetVirtualAddressMap`/runtime
/// service calls. KAOS calls no runtime services today, but the design doc (§2, §4)
/// keeps these regions mapped anyway, since platform/SMM code may still depend on them.
const EFI_MEMORY_RUNTIME: u64 = 0x8000_0000_0000_0000;

/// EFI memory types the design doc's §4 table marks "map" for firmware/platform
/// reasons, independent of the `EFI_MEMORY_RUNTIME` attribute bit.
const EFI_RESERVED_MEMORY_TYPE: u32 = 0;
const EFI_RUNTIME_SERVICES_CODE: u32 = 5;
const EFI_RUNTIME_SERVICES_DATA: u32 = 6;
const EFI_ACPI_MEMORY_NVS: u32 = 10;
const EFI_MEMORY_MAPPED_IO: u32 = 11;
const EFI_PAL_CODE: u32 = 13;

/// Phase 2 classifier: firmware/platform regions kept mapped explicitly instead of
/// relying on inherited firmware coverage (design doc §2/§4) — any region with the
/// `EFI_MEMORY_RUNTIME` attribute set, plus `RuntimeServicesCode`/`RuntimeServicesData`
/// (5/6), `ACPIMemoryNVS` (10), `Reserved` (0), `MemoryMappedIO` (11), and `PalCode`
/// (13). Disjoint from [`is_phase1_ram`] in normal memory maps (RAM vs. non-RAM types),
/// but the builder tolerates either classifier being run first — see the
/// `test_second_build_call_with_huge_page_collision_is_rejected`-style reuse pattern
/// exercised in `direct_map_test.rs`.
pub fn is_phase2_platform(entry: &UnifiedMemoryEntry) -> bool {
    (entry.attribute & EFI_MEMORY_RUNTIME) != 0
        || matches!(
            entry.memory_type,
            EFI_RESERVED_MEMORY_TYPE
                | EFI_RUNTIME_SERVICES_CODE
                | EFI_RUNTIME_SERVICES_DATA
                | EFI_ACPI_MEMORY_NVS
                | EFI_MEMORY_MAPPED_IO
                | EFI_PAL_CODE
        )
}

/// Builds a direct (VA == PA) map of every region `classify` accepts into the PML4
/// rooted at `pml4_phys`, using 2 MiB huge pages for 2-MiB-aligned bulk ranges when
/// `use_huge_pages` (falling back to 4 KiB for any unaligned head/tail, and for
/// everything when `use_huge_pages` is false).
///
/// Pure / host-testable: the only inputs are `regions`, `classify`, and `alloc_frame`.
/// No global VMM state is touched. Production code passes
/// [`super::page_table::alloc_frame_phys`] as `alloc_frame`; tests can pass a bump
/// allocator over a static buffer instead.
///
/// `pml4_phys` must already be an allocated, zeroed 4 KiB frame — the caller owns its
/// lifecycle, mirroring the existing `dst`/`dst_phys` split in
/// `build_kernel_pml4_from_firmware`. This function does **not** install a recursive
/// self-map slot: while this table is only being built and validated (not yet the
/// active CR3), the recursive window is irrelevant.
///
/// # Safety
/// `pml4_phys` and every physical address `alloc_frame` returns must be
/// dereferenceable as an identical virtual address for the whole call — true while the
/// original firmware/BIOS identity map is still active (i.e. before any CR3 switch away
/// from it). See the module-level rationale above.
pub unsafe fn build_direct_map<'a>(
    pml4_phys: u64,
    regions: impl IntoIterator<Item = &'a UnifiedMemoryEntry>,
    classify: impl Fn(&UnifiedMemoryEntry) -> bool,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    use_huge_pages: bool,
) -> Result<DirectMapStats, DirectMapError> {
    let mut stats = DirectMapStats::default();

    for region in regions {
        stats.regions_considered += 1;
        if !classify(region) {
            continue;
        }
        let end = region
            .start
            .checked_add(region.size)
            .ok_or(DirectMapError::RegionOverflow {
                start: region.start,
                size: region.size,
            })?;

        let mut cur = region.start;
        while cur < end {
            let remaining = end - cur;
            if use_huge_pages && cur % HUGE_PAGE_SIZE_2M == 0 && remaining >= HUGE_PAGE_SIZE_2M {
                map_2m_page(pml4_phys, cur, alloc_frame, &mut stats)?;
                stats.huge_2m_pages += 1;
                cur += HUGE_PAGE_SIZE_2M;
            } else {
                map_4k_page(pml4_phys, cur, alloc_frame, &mut stats)?;
                stats.small_4k_pages += 1;
                cur += PAGE_SIZE_U64;
            }
        }
        stats.regions_mapped += 1;
    }

    Ok(stats)
}

/// Ensures the PML4 -> PDPT -> PD path down to the PD table for `va` exists in the
/// tree rooted at `pml4_phys`, allocating/zeroing missing PDPT/PD frames via
/// `alloc_frame`. Returns the physical address of the PD table.
///
/// # Safety
/// Same contract as [`build_direct_map`].
unsafe fn ensure_pd_table(
    pml4_phys: u64,
    va: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    stats: &mut DirectMapStats,
) -> Result<u64, DirectMapError> {
    let pml4 = table_at(pml4_phys);
    let idx = pml4_index(va);
    if !table_entry(pml4, idx).present() {
        let phys = alloc_frame().ok_or(DirectMapError::OutOfScaffoldFrames)?;
        zero_phys_page(phys);
        (*entry_ptr(pml4, idx)).set_mapping(phys_to_pfn(phys), true, true, false);
        stats.pdpt_frames_allocated += 1;
    }
    let pdpt_phys = table_entry(pml4, idx).frame() * PAGE_SIZE_U64;

    let pdpt = table_at(pdpt_phys);
    let idx = pdp_index(va);
    let pdpte = table_entry(pdpt, idx);
    if pdpte.present() && pdpte.huge() {
        return Err(DirectMapError::HugePageCollision { va });
    }
    if !pdpte.present() {
        let phys = alloc_frame().ok_or(DirectMapError::OutOfScaffoldFrames)?;
        zero_phys_page(phys);
        (*entry_ptr(pdpt, idx)).set_mapping(phys_to_pfn(phys), true, true, false);
        stats.pd_frames_allocated += 1;
    }
    Ok(table_entry(pdpt, idx).frame() * PAGE_SIZE_U64)
}

/// Maps one 2 MiB huge page at physical/virtual address `pa` (identity map: VA == PA).
///
/// # Safety
/// Same contract as [`build_direct_map`].
unsafe fn map_2m_page(
    pml4_phys: u64,
    pa: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    stats: &mut DirectMapStats,
) -> Result<(), DirectMapError> {
    let pd_phys = ensure_pd_table(pml4_phys, pa, alloc_frame, stats)?;
    let pd = table_at(pd_phys);
    let idx = pd_index(pa);
    let existing = table_entry(pd, idx);
    if existing.present() {
        let existing_pa = existing.frame() * PAGE_SIZE_U64;
        if !existing.huge() {
            return Err(DirectMapError::HugePageCollision { va: pa });
        }
        if existing_pa != pa {
            return Err(DirectMapError::Overlap {
                va: pa,
                expected_pa: pa,
                existing_pa,
            });
        }
        return Ok(()); // identical, already installed - idempotent.
    }
    (*entry_ptr(pd, idx)).set_huge_mapping(pa, true, true, false);
    Ok(())
}

/// Maps one 4 KiB page at physical/virtual address `pa` (identity map: VA == PA).
///
/// # Safety
/// Same contract as [`build_direct_map`].
unsafe fn map_4k_page(
    pml4_phys: u64,
    pa: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    stats: &mut DirectMapStats,
) -> Result<(), DirectMapError> {
    let pd_phys = ensure_pd_table(pml4_phys, pa, alloc_frame, stats)?;
    let pd = table_at(pd_phys);
    let pd_idx = pd_index(pa);
    let pde = table_entry(pd, pd_idx);
    if pde.present() && pde.huge() {
        return Err(DirectMapError::HugePageCollision { va: pa });
    }
    if !pde.present() {
        let phys = alloc_frame().ok_or(DirectMapError::OutOfScaffoldFrames)?;
        zero_phys_page(phys);
        (*entry_ptr(pd, pd_idx)).set_mapping(phys_to_pfn(phys), true, true, false);
        stats.pt_frames_allocated += 1;
    }
    let pt_phys = table_entry(pd, pd_idx).frame() * PAGE_SIZE_U64;
    let pt = table_at(pt_phys);
    let pt_idx = pt_index(pa);
    let existing = table_entry(pt, pt_idx);
    if existing.present() {
        let existing_pa = existing.frame() * PAGE_SIZE_U64;
        if existing_pa != pa {
            return Err(DirectMapError::Overlap {
                va: pa,
                expected_pa: pa,
                existing_pa,
            });
        }
        return Ok(()); // identical, already installed - idempotent.
    }
    (*entry_ptr(pt, pt_idx)).set_mapping(phys_to_pfn(pa), true, true, false);
    Ok(())
}

/// Verifies every page of every region `classify` accepts resolves to its own
/// physical address (VA == PA) in the table rooted at `pml4_phys`, stepping by
/// whatever granularity is actually installed (2 MiB under a huge PD leaf, 4 KiB
/// otherwise) so a large-RAM validation stays a fast loop.
///
/// Pulled forward from the design doc's Phase 6 into Phase 1 itself: running this
/// immediately after [`build_direct_map`] turns a builder bug into a loud boot-time
/// panic instead of a much-later, misleading "SMM reset" symptom once a future phase
/// actually switches CR3 to this table.
///
/// # Safety
/// Same reachability contract as [`build_direct_map`]: every physical address touched
/// while walking must be dereferenceable as an identical virtual address.
pub unsafe fn validate_direct_map_coverage<'a>(
    pml4_phys: u64,
    regions: impl IntoIterator<Item = &'a UnifiedMemoryEntry>,
    classify: impl Fn(&UnifiedMemoryEntry) -> bool,
) -> Result<(), CoverageGap> {
    for region in regions {
        if !classify(region) {
            continue;
        }
        let end = region.start.saturating_add(region.size);
        let mut cur = region.start;
        while cur < end {
            match resolve_phys_via_root(pml4_phys, cur) {
                None => return Err(CoverageGap::Unmapped { va: cur }),
                Some((pa, _)) if pa != cur => {
                    return Err(CoverageGap::Mismatch {
                        va: cur,
                        expected_pa: cur,
                        got_pa: pa,
                    })
                }
                Some((_, step)) => cur += step,
            }
        }
    }
    Ok(())
}

/// Walks the whole tree at `pml4_phys`, releasing every present, non-huge
/// PML4/PDPT/PD/PT scaffold frame back to the PMM. Mirrors
/// `reserve_firmware_page_tables`'s walk (released instead of reserved) and needs no
/// stored frame list for the same reason `DirectMapStats` is aggregate-only (see the
/// module doc).
///
/// Used by the Phase 1 boot-time canary (build + validate + free, no CR3 switch yet)
/// so exercising this on every real boot has no lasting memory cost. A future phase
/// that actually switches CR3 to this table simply stops calling this function.
///
/// # Safety
/// `pml4_phys` and the whole tree reached from it must be dereferenceable as
/// identical virtual addresses, and must not be in use by anything else (in
/// particular: must not be the currently active CR3).
pub unsafe fn free_direct_map_tables(pml4_phys: u64) {
    pmm::with_pmm(|mgr| {
        mgr.release_pfn(phys_to_pfn(pml4_phys));
        let pml4 = table_at(pml4_phys);
        for i in 0..PT_ENTRIES {
            let e = table_entry(pml4, i);
            if !e.present() {
                continue;
            }
            let pdpt_phys = e.frame() * PAGE_SIZE_U64;
            mgr.release_pfn(e.frame());

            let pdpt = table_at(pdpt_phys);
            for j in 0..PT_ENTRIES {
                let e = table_entry(pdpt, j);
                if !e.present() || e.huge() {
                    continue;
                }
                let pd_phys = e.frame() * PAGE_SIZE_U64;
                mgr.release_pfn(e.frame());

                let pd = table_at(pd_phys);
                for k in 0..PT_ENTRIES {
                    let e = table_entry(pd, k);
                    if !e.present() || e.huge() {
                        continue;
                    }
                    mgr.release_pfn(e.frame());
                }
            }
        }
    });
}

/// Runs the Phase 1 boot-time canary: builds a complete kernel-owned direct map of
/// every RAM region described by the boot memory map, validates its coverage, then
/// frees it again — no CR3 switch happens here. Called from `vmm::init`, before the
/// (for now, still load-bearing) firmware-clone superset PML4 is built.
///
/// Runs unconditionally on every boot — both the BIOS and UEFI loaders publish a
/// `BootInfo` with a `UnifiedMemoryEntry` memory map — so a Phase 1 builder bug
/// surfaces as a loud, well-understood panic right here instead of a much later,
/// misleading real-hardware SMM-reset symptom once a future phase actually switches
/// CR3 to a table built this way. See the module doc above for why allocating scaffold
/// frames here, before this table is active, is safe.
///
/// # Safety
/// Must run before any CR3 switch away from the original firmware/BIOS-loader identity
/// map — true at its one call site in `vmm::init`, which calls this before its own
/// `write_cr3`.
pub unsafe fn run_boot_canary(debug_output: bool) {
    let boot_info_raw = BOOT_INFO_PTR.load(Ordering::Acquire);
    if boot_info_raw == 0 {
        // No BootInfo published (e.g. a unit-test kernel that never goes through the
        // normal boot path) - nothing to validate.
        return;
    }

    // SAFETY: `boot_info_raw` is the validated BootInfo pointer published by
    // `KernelMain` after checking its magic; the memory map it references is valid,
    // aligned, loader-populated memory (mirrors the existing access pattern in
    // `pmm::manager`).
    let boot_info = &*(boot_info_raw as *const crate::boot_info::BootInfo);
    let regions = core::slice::from_raw_parts(
        boot_info.memory_map_addr as *const UnifiedMemoryEntry,
        boot_info.memory_map_len as usize,
    );

    let pml4 = alloc_frame_phys_or_panic(
        "VMM: out of physical memory while allocating the Phase 1 direct-map canary PML4",
    );
    zero_phys_page(pml4);

    let mut alloc = alloc_frame_phys;
    let ram_stats = build_direct_map(pml4, regions.iter(), is_phase1_ram, &mut alloc, true)
        .unwrap_or_else(|e| panic!("Phase 1 direct-map build failed: {:?}", e));
    validate_direct_map_coverage(pml4, regions.iter(), is_phase1_ram)
        .unwrap_or_else(|e| panic!("Phase 1 direct-map coverage gap: {:?}", e));

    // Phase 2: reuse the same table for firmware/platform regions the design doc keeps
    // mapped explicitly. Distinct classifier, same builder — see is_phase2_platform's
    // doc for why running a second pass over the same tree is safe.
    let platform_stats =
        build_direct_map(pml4, regions.iter(), is_phase2_platform, &mut alloc, true)
            .unwrap_or_else(|e| panic!("Phase 2 direct-map build failed: {:?}", e));
    validate_direct_map_coverage(pml4, regions.iter(), is_phase2_platform)
        .unwrap_or_else(|e| panic!("Phase 2 direct-map coverage gap: {:?}", e));

    if debug_output {
        // Deliberately not `vmm_logln`/`vmm::debug_enabled()`: those read `VMM`'s
        // shared state via `with_vmm`, which `debug_assert!`s that the VMM is already
        // initialized — not yet true at this point in `vmm::init`. `debugln!` only
        // needs the serial port, which is up from very early in `KernelMain`.
        crate::debugln!(
            "VMM: Phase 1 direct-map canary OK: RAM={:?} platform={:?}",
            ram_stats,
            platform_stats
        );
    }

    free_direct_map_tables(pml4);
}
