//! Page-table logic tests (pure; no CR3 switch, no firmware, no QEMU devices).
//!
//! Covers the core of the UEFI `vmm::init` fix — `build_kernel_pml4_from_firmware` — plus
//! the virtual-address index math the recursive mapping and higher half rely on. See
//! `docs/vmm.md` §4. These run as a normal integration-test kernel but touch only in-memory
//! `PageTable` values, so they are fast and deterministic.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use kaos_kernel::memory::vmm::page_table::{
    build_kernel_pml4_from_firmware, phys_to_pfn, pml4_index, pd_index, pdp_index, pt_index,
    PageTable, PD_TABLE_BASE, PDP_TABLE_BASE, PML4_TABLE_ADDR, PT_ENTRIES, PT_TABLE_BASE,
    RECURSIVE_SLOT,
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
// build_kernel_pml4_from_firmware — the UEFI vmm::init fix
// ============================================================================

/// Contract: the kernel PML4 is a verbatim SUPERSET of the firmware PML4, plus a recursive
/// self-map at slot 511.
/// Given: a "firmware" PML4 filled with distinct entries in every slot.
/// When: build_kernel_pml4_from_firmware copies it into a fresh table for frame `dst_phys`.
/// Then: slots 0..=510 are byte-identical to the firmware table, and slot 511 is the
///       recursive self-map (present, writable, supervisor) pointing at `dst_phys`.
/// Failure Impact: regressing this re-introduces the minimal-map bug that reset real AMD
///       hardware at the CR3 switch (docs/vmm.md §4). Release-blocking.
#[test_case]
fn test_clone_copies_all_entries_and_sets_recursive() {
    let mut src = PageTable::new();
    let mut dst = PageTable::new();

    // Fill every firmware slot with a distinct, recognizable mapping.
    for i in 0..PT_ENTRIES {
        let pfn = (i as u64) + 0x10; // arbitrary non-zero, distinct per slot
        let present = true;
        let writable = i % 2 == 0;
        let user = i % 3 == 0;
        src.entries[i].set_mapping(pfn, present, writable, user);
    }

    // Arbitrary 4 KiB-aligned "physical" frame backing the destination table.
    let dst_phys: u64 = 0x0000_0007_FACE_0000;

    // SAFETY: both tables are valid, live stack objects; dst_phys stands in for dst's frame.
    unsafe {
        build_kernel_pml4_from_firmware(
            &src as *const PageTable,
            &mut dst as *mut PageTable,
            dst_phys,
        );
    }

    // Slots 0..=510 must be copied verbatim (raw bits identical).
    for i in 0..RECURSIVE_SLOT {
        assert_eq!(
            dst.entries[i].raw(),
            src.entries[i].raw(),
            "slot {i} must be a verbatim copy of the firmware entry"
        );
    }

    // Slot 511 must be the recursive self-map, NOT the copied firmware entry.
    let rec = dst.entries[RECURSIVE_SLOT];
    assert!(rec.present(), "recursive slot must be present");
    assert!(rec.writable(), "recursive slot must be writable");
    assert!(!rec.user(), "recursive slot must be supervisor-only");
    assert_eq!(
        rec.frame(),
        phys_to_pfn(dst_phys),
        "recursive slot must point at the PML4's own frame"
    );
    assert_ne!(
        rec.raw(),
        src.entries[RECURSIVE_SLOT].raw(),
        "recursive slot must override the firmware entry, not copy it"
    );
}

/// Contract: cloning does not mutate the source (firmware) table.
#[test_case]
fn test_clone_leaves_source_untouched() {
    let mut src = PageTable::new();
    for i in 0..PT_ENTRIES {
        src.entries[i].set_mapping((i as u64) + 1, true, true, false);
    }
    // Snapshot a few representative slots.
    let s0 = src.entries[0].raw();
    let s256 = src.entries[256].raw();
    let s511 = src.entries[511].raw();

    let mut dst = PageTable::new();
    // SAFETY: valid live tables.
    unsafe {
        build_kernel_pml4_from_firmware(&src as *const PageTable, &mut dst as *mut PageTable, 0x1000);
    }

    assert_eq!(src.entries[0].raw(), s0);
    assert_eq!(src.entries[256].raw(), s256);
    assert_eq!(src.entries[511].raw(), s511);
}

// ============================================================================
// Virtual-address index math (higher half + recursive mapping)
// ============================================================================

/// Contract: the higher-half base and kernel entry resolve to PML4 slot 256.
/// This is the slot the UEFI loader mirrors (PML4[0] -> PML4[256]) so the kernel can run
/// at 0xFFFF800000100000. Failure Impact: the higher half would map to the wrong slot.
#[test_case]
fn test_higher_half_indices() {
    assert_eq!(pml4_index(0xFFFF_8000_0000_0000), 256, "higher-half base");
    assert_eq!(pml4_index(0xFFFF_8000_0010_0000), 256, "kernel entry VA");
    // 0x100000 >> 12 == 0x100; & 0x1ff == 0x100.
    assert_eq!(pt_index(0xFFFF_8000_0010_0000), 0x100, "kernel entry PT index");
}

/// Contract: the recursive slot constant is 511, and the recursive self-VA decomposes into
/// all-511 indices at every level (the property that makes the recursive window work).
#[test_case]
fn test_recursive_indices() {
    assert_eq!(RECURSIVE_SLOT, 511);
    assert_eq!(pml4_index(0xFFFF_FFFF_FFFF_F000), 511);

    // PML4_TABLE_ADDR is the VA at which the PML4 maps itself: indices must be 511/511/511/511.
    assert_eq!(pml4_index(PML4_TABLE_ADDR), 511);
    assert_eq!(pdp_index(PML4_TABLE_ADDR), 511);
    assert_eq!(pd_index(PML4_TABLE_ADDR), 511);
    assert_eq!(pt_index(PML4_TABLE_ADDR), 511);
}

/// Contract: phys_to_pfn is a plain 4 KiB right-shift (PFN == addr / 4096).
#[test_case]
fn test_phys_to_pfn() {
    assert_eq!(phys_to_pfn(0x100000), 0x100);
    assert_eq!(phys_to_pfn(0), 0);
    assert_eq!(phys_to_pfn(0x0000_0007_FACE_0000), 0x0000_0007_FACE_0000 >> 12);
}

/// Contract: the recursive-window base constants are the canonical sign-extended addresses.
/// Failure Impact: the VMM's recursive table windows would point at the wrong VAs.
#[test_case]
fn test_recursive_window_constants() {
    assert_eq!(PML4_TABLE_ADDR, 0xFFFF_FFFF_FFFF_F000);
    assert_eq!(PDP_TABLE_BASE, 0xFFFF_FFFF_FFE0_0000);
    assert_eq!(PD_TABLE_BASE, 0xFFFF_FFFF_C000_0000);
    assert_eq!(PT_TABLE_BASE, 0xFFFF_FF80_0000_0000);
}
