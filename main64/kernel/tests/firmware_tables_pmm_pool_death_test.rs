//! Death test for the #63 R1 guard
//! `page_table::assert_no_active_table_frame_is_pmm_free`.
//!
//! That guard upholds the invariant that makes skipping `reserve_firmware_page_tables`
//! on the kernel-owned-table path safe: no frame of the currently-active
//! firmware/BIOS-loader page tables may be a free, allocatable PMM frame (otherwise the
//! direct-map builder could zero it out from under the live CR3 — see
//! `docs/todo_uefi_kernel_pagetables.md` §R1). On every real machine the invariant holds
//! by construction, so the guard never fires there; this test synthesizes a violation
//! (a *free* PMM frame handed to the guard as a table-tree root) and pins that the guard
//! reacts with a loud, named panic rather than passing silently.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::memory::pmm;
use kaos_kernel::memory::vmm::page_table;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    pmm::init(false);

    test_main();

    // The test must panic before reaching this point.
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let expected = "R1 invariant violated";
    if kaos_kernel::testing::panic_message_contains(info, expected) {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: handing the R1 guard a table-tree root frame that is a *free, allocatable*
/// PMM frame makes it panic naming the invariant, instead of letting a live firmware
/// page-table frame be silently reusable by the direct-map build.
/// Failure Impact: if the guard did not fire, a future regression that parked firmware
/// tables in usable RAM (or a PMM that pooled more memory types) would corrupt address
/// translation on the CR3 switch and hard-reset real hardware with no diagnostic —
/// release-blocking.
#[test_case]
fn test_r1_guard_panics_on_free_table_frame() {
    // Produce a frame that the PMM manages *and* considers free: allocate one (so it is
    // a genuine managed frame) and release it straight back to the pool.
    let pfn = pmm::with_pmm(|mgr| {
        let f = mgr.alloc_frame().expect("allocation should succeed");
        assert!(mgr.release_pfn(f.pfn), "release must succeed");
        f.pfn
    });

    // Sanity: it really is free/allocatable now.
    assert!(
        pmm::with_pmm(|mgr| mgr.is_pfn_free(pfn)),
        "the released frame must report free before the guard runs"
    );

    // Feed that free frame to the guard as the tree root. The guard checks the root
    // frame first and must panic on it — it never dereferences the frame, so its
    // contents are irrelevant.
    // SAFETY: `pfn << 12` names an identity-mapped low frame; the guard only reads the
    // PMM bitmap for it before panicking.
    unsafe { page_table::assert_no_active_table_frame_is_pmm_free(pfn << 12) };
}
