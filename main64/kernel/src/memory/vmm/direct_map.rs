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
    pml4_index, pt_index, resolve_phys_via_root, table_at, table_entry, write_cr3, zero_phys_page,
    HUGE_PAGE_SIZE_2M, PT_ENTRIES, RECURSIVE_SLOT,
};

/// Master switch for Phase 4 (#63): when `true`, `vmm::init` builds a genuinely
/// kernel-owned page-table hierarchy (Phase 1 RAM + Phase 2 platform regions + the
/// higher-half kernel mirror + the recursive self-map — see [`build_full_kernel_pml4`])
/// and switches CR3 to it instead of the firmware-clone superset
/// (`build_kernel_pml4_from_firmware`); `reserve_firmware_page_tables` is then skipped
/// in `main.rs` (the firmware's own sub-tables are no longer referenced and return to
/// the PMM).
///
/// Defaults to `false`. On real hardware, discarding the firmware's page tables when
/// switching CR3 has historically caused an immediate, exception-less hard reset — the
/// best-supported explanation is an asynchronous SMI faulting inside System Management
/// Mode once the firmware's own mappings are gone (see `docs/vmm.md` §4 and
/// `docs/boot_uefi.md` §3.9 for the full bisection writeup that established this).
/// This path is implemented and exercised in QEMU (`direct_map_full_switch_test.rs`),
/// where that SMM/SMI regression class is not reproducible at all — a QEMU pass here
/// does **not** prove the switch is safe on real hardware. Flip this to `true` only
/// after running the real AMD/UEFI-hardware smoke-test checklist in
/// `docs/boot_uefi.md`.
pub const USE_DIRECT_MAP_TABLE: bool = false;

/// PML4 slot for the higher-half kernel-image mirror (virtual `0xFFFF8000_00000000`).
const HIGHER_HALF_SLOT: usize = 256;

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
/// (5/6), `ACPIMemoryNVS` (10), `Reserved` (0), and `PalCode` (13). Disjoint from
/// [`is_phase1_ram`] in normal memory maps (RAM vs. non-RAM types), but the builder
/// tolerates either classifier being run first — see the
/// `test_second_build_call_with_huge_page_collision_is_rejected`-style reuse pattern
/// exercised in `direct_map_test.rs`.
///
/// **Deliberately excludes `MemoryMappedIO` (11)** — see [`is_mmio`]/
/// [`build_uc_direct_map`]: device memory needs uncacheable mappings, not the
/// write-back default this classifier's regions get from `map_2m_page`/`map_4k_page`.
pub fn is_phase2_platform(entry: &UnifiedMemoryEntry) -> bool {
    (entry.attribute & EFI_MEMORY_RUNTIME) != 0
        || matches!(
            entry.memory_type,
            EFI_RESERVED_MEMORY_TYPE
                | EFI_RUNTIME_SERVICES_CODE
                | EFI_RUNTIME_SERVICES_DATA
                | EFI_ACPI_MEMORY_NVS
                | EFI_PAL_CODE
        )
}

/// `EfiMemoryMappedIO` (11) classifier — split out of [`is_phase2_platform`] (#63
/// activation-path review, point 5) because MMIO must be mapped uncacheable (PCD set),
/// not the write-back default `map_2m_page`/`map_4k_page` apply to the rest of Phase 2.
/// Mapped via [`build_uc_direct_map`] instead of [`build_direct_map`].
pub fn is_mmio(entry: &UnifiedMemoryEntry) -> bool {
    entry.memory_type == EFI_MEMORY_MAPPED_IO
}

const EFI_LOADER_CODE: u32 = 1;
const EFI_LOADER_DATA: u32 = 2;

/// Loader-owned classifier: `EfiLoaderCode` (1) and `EfiLoaderData` (2) — the design
/// doc's §4 type table calls these out separately from Phase 2's firmware/platform
/// reasons ("BootInfo/map/PMM-meta live here; keep explicitly" for type 2, "otherwise
/// drop" for type 1). Kept as its own classifier rather than folded into
/// [`is_phase2_platform`] because the *reason* to map it is different: this is about
/// the loader's own allocations still being referenced by the kernel across the CR3
/// switch, not about platform/SMM needs.
///
/// Both types are mapped unconditionally here, not just type 2: on the UEFI path
/// `kaosldr_uefi` allocates the PMM-metadata region as `EfiLoaderData`
/// (`kaosldr_uefi/src/main.rs`'s `allocate_pages(0, 2, …)` call), and the loader's own
/// image (holding the `BootInfo`/memory-map statics) is typically allocated as one of
/// these two types by firmware convention — at boot time there is no cheap way to
/// prove nothing needed still lives in the `EfiLoaderCode` portion, and address space
/// is far cheaper than a fault or, worse, silent corruption from a missing mapping.
pub fn is_loader_owned(entry: &UnifiedMemoryEntry) -> bool {
    matches!(entry.memory_type, EFI_LOADER_CODE | EFI_LOADER_DATA)
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
        let raw_end =
            region
                .start
                .checked_add(region.size)
                .ok_or(DirectMapError::RegionOverflow {
                    start: region.start,
                    size: region.size,
                })?;
        // Real memory maps are not guaranteed page-aligned (e.g. QEMU/SeaBIOS reports
        // the classic low-memory region as [0x0, 0x9FC00) — 0x9FC00 is not a multiple
        // of 4 KiB). A PTE can only ever address a page-aligned frame, so round the
        // range outward to whole pages before mapping; otherwise the truncation
        // implicit in `phys_to_pfn`/`pt_index` (both `addr >> 12`) would silently
        // alias an unaligned `cur` onto the wrong page and could misreport a spurious
        // overlap against an adjacent region that rounds to the very same page.
        let start = region.start & !(PAGE_SIZE_U64 - 1);
        let end = (raw_end + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1);

        let mut cur = start;
        while cur < end {
            let remaining = end - cur;
            if use_huge_pages
                && cur.is_multiple_of(HUGE_PAGE_SIZE_2M)
                && remaining >= HUGE_PAGE_SIZE_2M
            {
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
    // Phase 5 (#63): NX on the whole identity/direct map. Safe because the kernel's
    // own code never executes through this identity mapping — it runs from the
    // separate higher-half mirror (PML4 slot 256), which maps the same physical
    // frames through different, unrelated PDPT/PD/PT entries and keeps whatever
    // permissions that chain already has (see `build_full_kernel_pml4`'s slot-256
    // copy). Marking the identity copy NX cannot affect what the CPU fetches from.
    let entry = &mut *entry_ptr(pd, idx);
    entry.set_huge_mapping(pa, true, true, false);
    entry.set_no_execute(true);
    Ok(())
}

/// Ensures the PD -> PT path for `va` exists, allocating/zeroing a missing PT frame via
/// `alloc_frame`. Returns the physical address of the PT table. Shared by
/// [`map_4k_page`] and [`map_4k_wc_page`] — the only difference between a normal and a
/// write-combining 4 KiB leaf is the flags on the final PTE, not how the path above it
/// is built.
///
/// # Safety
/// Same contract as [`build_direct_map`].
unsafe fn ensure_pt_table(
    pml4_phys: u64,
    va: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    stats: &mut DirectMapStats,
) -> Result<u64, DirectMapError> {
    let pd_phys = ensure_pd_table(pml4_phys, va, alloc_frame, stats)?;
    let pd = table_at(pd_phys);
    let pd_idx = pd_index(va);
    let pde = table_entry(pd, pd_idx);
    if pde.present() && pde.huge() {
        return Err(DirectMapError::HugePageCollision { va });
    }
    if !pde.present() {
        let phys = alloc_frame().ok_or(DirectMapError::OutOfScaffoldFrames)?;
        zero_phys_page(phys);
        (*entry_ptr(pd, pd_idx)).set_mapping(phys_to_pfn(phys), true, true, false);
        stats.pt_frames_allocated += 1;
    }
    Ok(table_entry(pd, pd_idx).frame() * PAGE_SIZE_U64)
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
    let pt_phys = ensure_pt_table(pml4_phys, pa, alloc_frame, stats)?;
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
    // Phase 5 (#63): NX on the whole identity/direct map — see map_2m_page's comment
    // for why this cannot affect the kernel's own (higher-half) code execution.
    let entry = &mut *entry_ptr(pt, pt_idx);
    entry.set_mapping(phys_to_pfn(pa), true, true, false);
    entry.set_no_execute(true);
    Ok(())
}

/// Maps `[base, base + size)` identity (VA == PA) as write-combining, NX, 4 KiB pages
/// into the tree rooted at `pml4_phys` — Phase 3 of #63: the GOP framebuffer must be
/// mapped explicitly in the kernel-owned table instead of relying on inherited firmware
/// coverage (design doc problem P4). `base` is rounded down and `size` up to page
/// boundaries, matching `mapping.rs`'s existing `configure_wc_mapping` PAT1 convention
/// (bit 3 = PWT, bit 4 = PCD; PAT1 is configured for Write-Combining by the PAT MSR
/// setup in `main.rs`'s `map_framebuffer`) — this function assumes that PAT
/// configuration is already in place, it only sets the PWT/PCD bits on each leaf.
/// Always 4 KiB granularity: framebuffers are rarely 2 MiB-aligned, and — unlike bulk
/// RAM — there is no benefit to huge pages for a single MMIO range mapped once at boot.
///
/// # Safety
/// Same contract as [`build_direct_map`]: `pml4_phys` and every physical address
/// `alloc_frame` returns must be dereferenceable as an identical virtual address for the
/// whole call.
pub unsafe fn map_wc_range(
    pml4_phys: u64,
    base: u64,
    size: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
) -> Result<(), DirectMapError> {
    let start = base & !(PAGE_SIZE_U64 - 1);
    let end = base
        .checked_add(size)
        .map(|end| (end + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1))
        .ok_or(DirectMapError::RegionOverflow { start: base, size })?;

    let mut stats = DirectMapStats::default();
    let mut cur = start;
    while cur < end {
        map_4k_wc_page(pml4_phys, cur, alloc_frame, &mut stats)?;
        cur += PAGE_SIZE_U64;
    }
    Ok(())
}

/// Maps one 4 KiB write-combining, NX leaf at physical/virtual address `pa` (identity
/// map: VA == PA). See [`map_wc_range`] for the PAT/PWT/PCD convention.
///
/// # Safety
/// Same contract as [`build_direct_map`].
unsafe fn map_4k_wc_page(
    pml4_phys: u64,
    pa: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    stats: &mut DirectMapStats,
) -> Result<(), DirectMapError> {
    let pt_phys = ensure_pt_table(pml4_phys, pa, alloc_frame, stats)?;
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
    let entry = &mut *entry_ptr(pt, pt_idx);
    entry.set_mapping(phys_to_pfn(pa), true, true, false);
    entry.set_pwt(true);
    entry.set_pcd(false);
    entry.set_no_execute(true);
    Ok(())
}

/// Builds an uncacheable (PCD set, PWT clear), NX, 4 KiB-only direct map of every
/// region `classify` accepts — the MMIO counterpart to [`map_wc_range`]'s
/// write-combining framebuffer mapping (#63 activation-path review, point 5). Always
/// 4 KiB granularity, for the same reason `map_wc_range` is: MMIO ranges are rarely
/// 2 MiB-aligned and gain nothing from huge pages for what is usually a handful of
/// mappings created once at boot.
///
/// # Safety
/// `pml4_phys` and every physical address `alloc_frame` returns must be
/// dereferenceable as an identical virtual address for the whole call — same contract
/// as [`build_direct_map`].
pub unsafe fn build_uc_direct_map<'a>(
    pml4_phys: u64,
    regions: impl IntoIterator<Item = &'a UnifiedMemoryEntry>,
    classify: impl Fn(&UnifiedMemoryEntry) -> bool,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
) -> Result<DirectMapStats, DirectMapError> {
    let mut stats = DirectMapStats::default();

    for region in regions {
        stats.regions_considered += 1;
        if !classify(region) {
            continue;
        }
        let raw_end =
            region
                .start
                .checked_add(region.size)
                .ok_or(DirectMapError::RegionOverflow {
                    start: region.start,
                    size: region.size,
                })?;
        // Round outward to page boundaries — see build_direct_map's identical comment
        // for why (real memory-map regions are not guaranteed page-aligned).
        let start = region.start & !(PAGE_SIZE_U64 - 1);
        let end = (raw_end + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1);

        let mut cur = start;
        while cur < end {
            map_4k_uc_page(pml4_phys, cur, alloc_frame, &mut stats)?;
            stats.small_4k_pages += 1;
            cur += PAGE_SIZE_U64;
        }
        stats.regions_mapped += 1;
    }

    Ok(stats)
}

/// Maps one 4 KiB uncacheable, NX leaf at physical/virtual address `pa` (identity map:
/// VA == PA). See [`build_uc_direct_map`] for the PCD/PWT convention.
///
/// # Safety
/// Same contract as [`build_direct_map`].
unsafe fn map_4k_uc_page(
    pml4_phys: u64,
    pa: u64,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
    stats: &mut DirectMapStats,
) -> Result<(), DirectMapError> {
    let pt_phys = ensure_pt_table(pml4_phys, pa, alloc_frame, stats)?;
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
    let entry = &mut *entry_ptr(pt, pt_idx);
    entry.set_mapping(phys_to_pfn(pa), true, true, false);
    entry.set_pcd(true);
    entry.set_no_execute(true);
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
        validate_byte_range_coverage(pml4_phys, region.start, region.size)?;
    }
    Ok(())
}

/// Verifies every page of `[start, start + size)` resolves to its own physical address
/// (VA == PA) in the table rooted at `pml4_phys`. Shared stepping logic behind
/// [`validate_direct_map_coverage`] (which iterates `UnifiedMemoryEntry` regions) and
/// [`validate_essential_boot_addresses`] (which checks specific byte ranges that have
/// no `UnifiedMemoryEntry` of their own to iterate).
///
/// # Safety
/// Same reachability contract as [`build_direct_map`].
unsafe fn validate_byte_range_coverage(
    pml4_phys: u64,
    start: u64,
    size: u64,
) -> Result<(), CoverageGap> {
    let end = start.saturating_add(size);
    let mut cur = start;
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
    Ok(())
}

/// Validates that a handful of addresses the kernel unconditionally dereferences after
/// a CR3 switch — the `BootInfo` structure itself, its memory-map array, and (if
/// present) the PMM-metadata region — resolve correctly in the table rooted at
/// `pml4_phys`.
///
/// Deliberately independent of [`is_phase1_ram`]/[`is_phase2_platform`]/
/// [`is_loader_owned`]: those classify by EFI memory *type*, and a future bug or
/// change in one of them should not silently reopen a gap in exactly the addresses
/// that matter most — this check does not care which classifier (if any) covers them,
/// only whether they resolve.
///
/// # Safety
/// Same reachability contract as [`build_direct_map`]; `boot_info` must be the
/// currently published `BootInfo` (still reachable via the old identity map).
pub unsafe fn validate_essential_boot_addresses(
    pml4_phys: u64,
    boot_info: &crate::boot_info::BootInfo,
) -> Result<(), CoverageGap> {
    validate_byte_range_coverage(
        pml4_phys,
        boot_info as *const _ as u64,
        core::mem::size_of::<crate::boot_info::BootInfo>() as u64,
    )?;

    validate_byte_range_coverage(
        pml4_phys,
        boot_info.memory_map_addr,
        boot_info.memory_map_len as u64 * core::mem::size_of::<UnifiedMemoryEntry>() as u64,
    )?;

    if boot_info.pmm_metadata_base != 0 {
        validate_byte_range_coverage(
            pml4_phys,
            boot_info.pmm_metadata_base,
            boot_info.pmm_metadata_size,
        )?;
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
            // Skip the higher-half mirror (borrowed verbatim from whatever PML4 was
            // active when `build_full_kernel_pml4` ran — freeing it would release
            // frames a *different*, still-live table still depends on) and the
            // recursive self-map (points back at `pml4_phys` itself, already released
            // above; walking it as if it were a PDPT would misinterpret this table's
            // own top-level entries as a level down and release unrelated frames).
            // Both slots are no-ops for a plain `build_direct_map`-only canary table
            // (neither is ever set there), so this is a pure safety fix, not a
            // behavior change for that use case.
            if i == HIGHER_HALF_SLOT || i == RECURSIVE_SLOT {
                continue;
            }
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

    // Loader-owned regions (EfiLoaderCode/EfiLoaderData): on the UEFI path the PMM
    // metadata region and the loader's own BootInfo/memory-map statics typically live
    // here — see is_loader_owned's doc. Neither Phase 1 nor Phase 2 classifies these.
    let loader_stats = build_direct_map(pml4, regions.iter(), is_loader_owned, &mut alloc, true)
        .unwrap_or_else(|e| panic!("Loader-owned direct-map build failed: {:?}", e));
    validate_direct_map_coverage(pml4, regions.iter(), is_loader_owned)
        .unwrap_or_else(|e| panic!("Loader-owned direct-map coverage gap: {:?}", e));

    // MMIO (EfiMemoryMappedIO, 11): mapped uncacheable via its own builder, split out
    // of the Phase 2 pass above — see is_mmio's doc.
    let mmio_stats = build_uc_direct_map(pml4, regions.iter(), is_mmio, &mut alloc)
        .unwrap_or_else(|e| panic!("MMIO direct-map build failed: {:?}", e));
    validate_direct_map_coverage(pml4, regions.iter(), is_mmio)
        .unwrap_or_else(|e| panic!("MMIO direct-map coverage gap: {:?}", e));

    // Independent of the classifiers above: confirm the specific addresses the kernel
    // actually dereferences (BootInfo itself, its memory map, the PMM metadata region)
    // resolve in the new table, regardless of which classifier happens to cover them.
    validate_essential_boot_addresses(pml4, boot_info)
        .unwrap_or_else(|e| panic!("Essential boot address coverage gap: {:?}", e));

    if debug_output {
        // Deliberately not `vmm_logln`/`vmm::debug_enabled()`: those read `VMM`'s
        // shared state via `with_vmm`, which `debug_assert!`s that the VMM is already
        // initialized — not yet true at this point in `vmm::init`. `debugln!` only
        // needs the serial port, which is up from very early in `KernelMain`.
        crate::debugln!(
            "VMM: Phase 1 direct-map canary OK: RAM={:?} platform={:?} loader={:?} mmio={:?}",
            ram_stats,
            platform_stats,
            loader_stats,
            mmio_stats
        );
    }

    free_direct_map_tables(pml4);
}

/// Builds a genuinely kernel-owned PML4: Phase 1 (RAM) + Phase 2 (firmware/platform) +
/// loader-owned (EfiLoaderCode/EfiLoaderData — see [`is_loader_owned`]) regions mapped
/// explicitly (RAM/platform both NX — Phase 5's "data + direct map: NX", see
/// `map_2m_page`/`map_4k_page`), the GOP framebuffer mapped write-combining + NX via
/// [`map_wc_range`] when one is present (Phase 3), the higher-half kernel-image mirror
/// (PML4 slot 256) copied verbatim from `old_pml4_phys`'s slot 256 (so the kernel's own
/// code/data stays reachable after a future CR3 switch — this table never rebuilds
/// that mapping itself, it only borrows the existing chain), and the recursive
/// self-map installed at slot 511. Matches the design doc's Phase 4 checklist
/// ("P1 + P2 + P3 + slot 511 + slot 256" — see `docs/todo_uefi_kernel_pagetables.md`).
///
/// **Phase 5 scope note:** because slot 256 is copied verbatim rather than rebuilt,
/// this does *not* yet enforce "kernel `.text` is RO+X" (the other half of Phase 5) —
/// that chain keeps whatever permissions it already had (today, RWX, matching problem
/// P1 in the design doc). Enforcing W^X on the kernel image itself requires page-
/// aligning `.text`/`.rodata`/`.data` in `link.ld` (they are currently packed
/// contiguously with no boundary symbols) and rebuilding slot 256 at 4 KiB granularity
/// for just the kernel-image range — a separate, higher-blast-radius change (the linker
/// script affects every boot configuration, not just `USE_DIRECT_MAP_TABLE`) left as
/// follow-up work; see the tracking issue.
///
/// Does **not** switch CR3 — see [`switch_to_direct_map`] for that, and
/// [`USE_DIRECT_MAP_TABLE`] for why this is not wired in by default.
///
/// # Safety
/// Same reachability contract as [`build_direct_map`], plus: `old_pml4_phys` must be
/// the physical address of the currently active PML4, so the slot 256 copy is coherent
/// with what the CPU is actually executing from.
pub unsafe fn build_full_kernel_pml4(
    old_pml4_phys: u64,
    regions: &[UnifiedMemoryEntry],
    boot_info: &crate::boot_info::BootInfo,
    alloc_frame: &mut dyn FnMut() -> Option<u64>,
) -> Result<
    (
        u64,
        DirectMapStats,
        DirectMapStats,
        DirectMapStats,
        DirectMapStats,
    ),
    DirectMapError,
> {
    let new_pml4 = alloc_frame().ok_or(DirectMapError::OutOfScaffoldFrames)?;
    zero_phys_page(new_pml4);

    let ram_stats = build_direct_map(new_pml4, regions.iter(), is_phase1_ram, alloc_frame, true)?;
    validate_direct_map_coverage(new_pml4, regions.iter(), is_phase1_ram)
        .unwrap_or_else(|e| panic!("Phase 4 direct-map RAM coverage gap: {:?}", e));

    let platform_stats = build_direct_map(
        new_pml4,
        regions.iter(),
        is_phase2_platform,
        alloc_frame,
        true,
    )?;
    validate_direct_map_coverage(new_pml4, regions.iter(), is_phase2_platform)
        .unwrap_or_else(|e| panic!("Phase 4 direct-map platform coverage gap: {:?}", e));

    // Loader-owned regions (EfiLoaderCode/EfiLoaderData) — see is_loader_owned's doc:
    // on the UEFI path the PMM-metadata region and the loader's own BootInfo/memory-map
    // statics typically live here, and neither classifier above covers it.
    let loader_stats =
        build_direct_map(new_pml4, regions.iter(), is_loader_owned, alloc_frame, true)?;
    validate_direct_map_coverage(new_pml4, regions.iter(), is_loader_owned)
        .unwrap_or_else(|e| panic!("Phase 4 loader-owned direct-map coverage gap: {:?}", e));

    // MMIO (EfiMemoryMappedIO, 11): mapped uncacheable via its own builder, split out
    // of the Phase 2 pass above — see is_mmio's doc.
    let mmio_stats = build_uc_direct_map(new_pml4, regions.iter(), is_mmio, alloc_frame)?;
    validate_direct_map_coverage(new_pml4, regions.iter(), is_mmio)
        .unwrap_or_else(|e| panic!("Phase 4 MMIO direct-map coverage gap: {:?}", e));

    // Independent of the three classifiers above: confirm the specific addresses the
    // kernel actually dereferences after the switch resolve, regardless of which (if
    // any) classifier happens to cover them today.
    validate_essential_boot_addresses(new_pml4, boot_info)
        .unwrap_or_else(|e| panic!("Phase 4 essential boot address coverage gap: {:?}", e));

    // Phase 3: map the GOP framebuffer explicitly (write-combining + NX), instead of
    // relying on inherited firmware coverage — design doc problem P4. Only present on
    // a graphics-mode boot (BIOS text-mode/no-framebuffer boots leave `base_address`
    // at 0). This was previously documented as "the caller's responsibility" but never
    // actually wired in anywhere — fixed here, the only production call site.
    if boot_info.video_type == crate::boot_info::VideoModeType::Framebuffer
        && boot_info.fb_info.base_address != 0
    {
        map_wc_range(
            new_pml4,
            boot_info.fb_info.base_address,
            boot_info.fb_info.size as u64,
            alloc_frame,
        )
        .unwrap_or_else(|e| panic!("Phase 4 framebuffer direct-map failed: {:?}", e));
    }

    // Higher-half kernel-image mirror: copy verbatim from the currently active PML4,
    // rather than rebuilding it — the kernel's own image lives wherever the loader put
    // it, and that chain already works.
    let old_pml4_table = table_at(old_pml4_phys);
    let new_pml4_table = table_at(new_pml4);
    *entry_ptr(new_pml4_table, HIGHER_HALF_SLOT) = table_entry(old_pml4_table, HIGHER_HALF_SLOT);

    // Recursive self-map, exactly like `build_kernel_pml4_from_firmware`'s slot 511.
    (*entry_ptr(new_pml4_table, RECURSIVE_SLOT)).set_mapping(
        phys_to_pfn(new_pml4),
        true,
        true,
        false,
    );

    Ok((
        new_pml4,
        ram_stats,
        platform_stats,
        loader_stats,
        mmio_stats,
    ))
}

/// Builds a full kernel-owned PML4 from the current boot memory map (see
/// [`build_full_kernel_pml4`]) and switches CR3 to it, returning the new PML4's
/// physical address.
///
/// After this returns, firmware/BIOS-loader sub-tables are no longer referenced by the
/// active table — the caller must not also reserve them from the PMM
/// (`reserve_firmware_page_tables`), since they should return to the pool of usable
/// frames instead of staying permanently reserved. `vmm::init` is the only call site,
/// gated by [`USE_DIRECT_MAP_TABLE`].
///
/// # Safety
/// Must run before any other write to CR3 in this boot, with the same reachability
/// contract as [`build_direct_map`] (the old identity map must still be active while
/// this builds the new table). `old_pml4_phys` must be the physical address of the
/// currently active PML4.
pub unsafe fn switch_to_direct_map(old_pml4_phys: u64) -> u64 {
    let boot_info_raw = BOOT_INFO_PTR.load(Ordering::Acquire);
    assert_ne!(
        boot_info_raw, 0,
        "switch_to_direct_map requires a published BootInfo"
    );

    // SAFETY: see `run_boot_canary`'s identical access pattern above.
    let boot_info = &*(boot_info_raw as *const crate::boot_info::BootInfo);
    let regions = core::slice::from_raw_parts(
        boot_info.memory_map_addr as *const UnifiedMemoryEntry,
        boot_info.memory_map_len as usize,
    );

    let mut alloc = alloc_frame_phys;
    let (new_pml4, ram_stats, platform_stats, loader_stats, mmio_stats) =
        build_full_kernel_pml4(old_pml4_phys, regions, boot_info, &mut alloc)
            .unwrap_or_else(|e| panic!("Phase 4 direct-map build failed: {:?}", e));

    crate::debugln!(
        "VMM: Phase 4 switching CR3 to kernel-owned direct map: RAM={:?} platform={:?} loader={:?} mmio={:?}",
        ram_stats,
        platform_stats,
        loader_stats,
        mmio_stats
    );

    write_cr3(new_pml4);
    new_pml4
}
