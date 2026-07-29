//! Pure page-table entry / explicit-root-walk tests (no CR3 switch, no firmware, no
//! recursive mapping involved).
//!
//! These pin the low-level building blocks the direct-map builder (`memory::vmm::direct_map`,
//! part of #63 — kernel-owned page tables on the UEFI path) depends on:
//! - `PageTableEntry::set_huge` / `set_huge_mapping`, the 2 MiB PD-leaf creation counterpart
//!   to the existing 4 KiB-only `set_mapping`.
//! - `resolve_phys_via_root`, which walks an explicit (not necessarily active) PML4 by
//!   physical address, unlike the recursive-mapping helpers that only ever see the
//!   currently active CR3.
//!
//! All tables here are synthetic statics; their own addresses stand in for "physical"
//! addresses exactly like `pmm_uefi_test.rs`'s `META_BUF` does — the functions under
//! test only ever dereference whatever address they are given as a raw pointer, so
//! this works whether that address is truly physical or just the address of a
//! kernel-image static (no CR3 switch is involved, so this needs no `vmm::init`).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut};

use kaos_kernel::arch::constants::PAGE_SIZE_U64;
use kaos_kernel::memory::pmm::types::virt_to_phys;
use kaos_kernel::memory::vmm::page_table::{
    pd_index, pdp_index, phys_to_pfn, pml4_index, pt_index, resolve_phys_via_root, PageTable,
    HUGE_PAGE_SIZE_2M,
};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

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
// Synthetic table hierarchy shared by the tests below. Zeroed at the start of
// each test that uses it, so tests do not leak state into each other.
// ============================================================================

static mut PML4: PageTable = PageTable::new();
static mut PDPT: PageTable = PageTable::new();
static mut PD: PageTable = PageTable::new();
static mut PT: PageTable = PageTable::new();

/// Links PML4 -> PDPT -> PD (all present, non-huge) for `va`'s PML4/PDP indices, and
/// returns the physical addresses of the three tables.
///
/// The tables are kernel-image statics, so their raw `as u64` address is a *virtual*,
/// sign-extended higher-half address (e.g. `0xFFFF8000_00xxxxxx`) — bits above 47 would
/// be silently dropped by `set_frame`'s `ENTRY_FRAME_MASK` (48-bit physical address
/// space) if written directly into a page-table entry, corrupting the roundtrip.
/// `virt_to_phys` converts to the matching low physical address (still resolvable via
/// `table_at`, because that same physical range is *also* covered by the low identity
/// map that is active pre-`vmm::init` — see `docs/boot_uefi.md` §3), exactly the way
/// production code (e.g. `build_kernel_pml4_from_firmware`) only ever deals in real
/// physical addresses, never sign-extended virtual ones.
///
/// # Safety
/// Caller must not alias `&mut` references to `PML4`/`PDPT`/`PD` concurrently.
unsafe fn link_pml4_pdpt_pd(va: u64) -> (u64, u64, u64) {
    (*addr_of_mut!(PML4)).zero();
    (*addr_of_mut!(PDPT)).zero();
    (*addr_of_mut!(PD)).zero();
    (*addr_of_mut!(PT)).zero();

    let pml4_phys = virt_to_phys(addr_of!(PML4) as u64);
    let pdpt_phys = virt_to_phys(addr_of!(PDPT) as u64);
    let pd_phys = virt_to_phys(addr_of!(PD) as u64);

    (*addr_of_mut!(PML4)).entries[pml4_index(va)].set_mapping(
        phys_to_pfn(pdpt_phys),
        true,
        true,
        false,
    );
    (*addr_of_mut!(PDPT)).entries[pdp_index(va)].set_mapping(
        phys_to_pfn(pd_phys),
        true,
        true,
        false,
    );

    (pml4_phys, pdpt_phys, pd_phys)
}

/// Contract: `set_huge`/`set_huge_mapping` write frame + huge bit + permission bits, and
/// `set_huge(false)` clears only the huge bit, leaving the rest of the entry intact.
/// Failure Impact: a wrong bit pattern would either corrupt the frame address encoded in
/// a huge leaf, or fail to mark it huge (CPU would misinterpret the entry as a sub-table
/// pointer) — release-blocking for the direct-map builder.
#[test_case]
fn test_set_huge_mapping_roundtrip() {
    // SAFETY: single-threaded test context, no concurrent access to PD.
    unsafe {
        (*addr_of_mut!(PD)).zero();
        let entry = &mut (*addr_of_mut!(PD)).entries[5];

        let phys = 0x0000_0040_0000_u64; // 4 MiB, 2 MiB aligned
        entry.set_huge_mapping(phys, true, true, false);

        assert!(entry.present());
        assert!(entry.writable());
        assert!(!entry.user());
        assert!(entry.huge());
        assert_eq!(entry.frame() * PAGE_SIZE_U64, phys);

        let before = entry.raw();
        entry.set_huge(false);
        assert!(!entry.huge());
        // Only bit 7 (ENTRY_HUGE) should have changed.
        assert_eq!(entry.raw(), before & !(1u64 << 7));
    }
}

// Note: the misaligned-frame panic contract for `set_huge_mapping` is covered by the
// dedicated death test in `huge_mapping_align_death_test.rs` — this custom
// `#[test_case]` harness has no `#[should_panic]` support (that's a libtest-only
// attribute; there is no `test` crate linked into this `no_std` binary), so a panic
// contract needs its own binary with a panic handler that turns the expected panic
// into `QemuExitCode::Success`, exactly like `page_fault_death_test.rs`.

/// Contract: `resolve_phys_via_root` resolves a VA through a 4 KiB PT leaf in an explicit
/// (not active) root, returning the exact physical address including the page offset.
/// Failure Impact: the Phase 1 direct-map coverage check (`direct_map::
/// validate_direct_map_coverage`) relies on this to detect gaps in a not-yet-active table;
/// a wrong result would let a real coverage gap through undetected.
#[test_case]
fn test_resolve_phys_via_root_4k_leaf() {
    // VA with pml4_idx=0, pdp_idx=0, pd_idx=0, pt_idx=3, offset=0x123.
    let va: u64 = (3 << 12) | 0x123;

    // SAFETY: single-threaded test context; tables are only reachable from this function
    // for the duration of the call, and their addresses are valid live pointers (kernel
    // image statics), satisfying resolve_phys_via_root's reachability contract.
    unsafe {
        let (pml4_phys, _pdpt_phys, pd_phys) = link_pml4_pdpt_pd(va);
        let pt_phys = virt_to_phys(addr_of!(PT) as u64);

        (*addr_of_mut!(PD)).entries[pd_index(va)].set_mapping(
            phys_to_pfn(pt_phys),
            true,
            true,
            false,
        );

        let leaf_phys = 0x0000_0000_00AB_C000_u64; // 4 KiB aligned target
        (*addr_of_mut!(PT)).entries[pt_index(va)].set_mapping(
            phys_to_pfn(leaf_phys),
            true,
            true,
            false,
        );

        let resolved = resolve_phys_via_root(pml4_phys, va);
        assert_eq!(resolved, Some((leaf_phys + 0x123, PAGE_SIZE_U64)));
        let _ = pd_phys; // silence unused-binding warning if the field is not needed further
    }
}

/// Contract: `resolve_phys_via_root` resolves a VA through a 2 MiB PD huge leaf, without
/// descending into a (nonexistent) PT.
/// Failure Impact: same as above, but for the 2 MiB bulk-mapping path the direct-map
/// builder uses for the overwhelming majority of RAM.
#[test_case]
fn test_resolve_phys_via_root_2m_leaf() {
    // VA with pml4_idx=0, pdp_idx=0, pd_idx=1 (2 MiB aligned), offset=0x4000.
    let va: u64 = HUGE_PAGE_SIZE_2M | 0x4000;

    // SAFETY: see test_resolve_phys_via_root_4k_leaf.
    unsafe {
        let (pml4_phys, _pdpt_phys, _pd_phys) = link_pml4_pdpt_pd(va);

        let leaf_phys = 0x0000_0004_0000_0000_u64; // 2 MiB aligned target
        (*addr_of_mut!(PD)).entries[pd_index(va)].set_huge_mapping(leaf_phys, true, true, false);

        let resolved = resolve_phys_via_root(pml4_phys, va);
        assert_eq!(resolved, Some((leaf_phys + 0x4000, HUGE_PAGE_SIZE_2M)));
    }
}

/// Contract: `resolve_phys_via_root` resolves a VA through a 1 GiB PDPT huge leaf,
/// without descending into a (nonexistent) PD/PT, returning `(pa, 1 GiB)`.
/// Failure Impact: firmware page tables commonly use 1 GiB pages, and the coverage
/// validator walks whatever the active firmware/loader table actually contains; a wrong
/// 1 GiB result (or a spurious descent) would misreport coverage for gigabyte-sized
/// ranges.
#[test_case]
fn test_resolve_phys_via_root_1g_leaf() {
    const GIB: u64 = 1024 * 1024 * 1024;
    // VA with pml4_idx=0, pdp_idx=1 (1 GiB aligned), offset=0x5000.
    let va: u64 = GIB | 0x5000;

    // SAFETY: see test_resolve_phys_via_root_4k_leaf. Only PML4 -> PDPT is linked; the
    // PDPT entry is installed as a 1 GiB huge leaf, so no PD/PT is involved.
    unsafe {
        (*addr_of_mut!(PML4)).zero();
        (*addr_of_mut!(PDPT)).zero();

        let pml4_phys = virt_to_phys(addr_of!(PML4) as u64);
        let pdpt_phys = virt_to_phys(addr_of!(PDPT) as u64);

        (*addr_of_mut!(PML4)).entries[pml4_index(va)].set_mapping(
            phys_to_pfn(pdpt_phys),
            true,
            true,
            false,
        );

        // 1 GiB-aligned target installed directly as a huge PDPT leaf (the huge bit at
        // PDPT level means a 1 GiB page). 1 GiB-aligned satisfies `set_huge_mapping`'s
        // 2 MiB-alignment assertion.
        let leaf_phys = 0x0000_0003_0000_0000_u64;
        (*addr_of_mut!(PDPT)).entries[pdp_index(va)].set_huge_mapping(leaf_phys, true, true, false);

        let resolved = resolve_phys_via_root(pml4_phys, va);
        assert_eq!(resolved, Some((leaf_phys + 0x5000, GIB)));
    }
}

/// Contract: `resolve_phys_via_root` returns `None` for a VA whose path is not fully
/// present (here: the PD entry itself is absent).
/// Failure Impact: a coverage check that can't distinguish "unmapped" from "mapped" would
/// silently accept holes in the direct map.
#[test_case]
fn test_resolve_phys_via_root_unmapped_returns_none() {
    let va: u64 = (7 << 12) | 0x10;

    // SAFETY: see test_resolve_phys_via_root_4k_leaf. Deliberately do not populate the PD
    // entry, leaving the path non-present at the PD level.
    unsafe {
        let (pml4_phys, _pdpt_phys, _pd_phys) = link_pml4_pdpt_pd(va);
        assert_eq!(resolve_phys_via_root(pml4_phys, va), None);
    }
}
