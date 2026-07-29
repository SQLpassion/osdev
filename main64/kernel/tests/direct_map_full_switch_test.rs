//! Phase 4 (#63): exercises the actual CR3 switch to a fully kernel-owned page table
//! (`direct_map::switch_to_direct_map`) in QEMU, calling the switch entry point directly
//! (production reaches the same code from `vmm::init` on every boot that publishes a
//! `BootInfo`).
//!
//! This is the highest-risk operation in the whole #63 effort: discarding the
//! firmware's page tables when switching CR3 has historically caused an immediate,
//! exception-less hard reset on real AMD/UEFI hardware (an asynchronous SMI faulting
//! inside SMM once the firmware's own mappings are gone — see `docs/vmm.md` §4 and
//! `docs/boot_uefi.md` §3.9). That regression class is **not reproducible in QEMU at
//! all**, so a green run here proves the mapping/switch mechanics are correct, but
//! makes NO claim about real-hardware safety — that was cleared separately by the
//! real-hardware smoke-test checklist in `docs/boot_uefi.md`.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use kaos_kernel::arch::interrupts;
use kaos_kernel::boot_info::BOOT_INFO_PTR;
use kaos_kernel::memory::pmm;
use kaos_kernel::memory::vmm::direct_map;
use kaos_kernel::memory::vmm::page_table::read_cr3;

const BOOT_INFO_MAGIC: u64 = 0x4B41_4F53_5F42_4F4F; // "KAOS_BOO"

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(boot_info_raw: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // SAFETY: mirrors main.rs's own magic check before trusting the loader-provided
    // pointer; `boot_info_raw` is the same RDI value every KernelMain receives.
    let has_boot_info =
        boot_info_raw != 0 && unsafe { *(boot_info_raw as *const u64) } == BOOT_INFO_MAGIC;
    if has_boot_info {
        // Publish the REAL loader-provided BootInfo (not a synthetic one) so this test
        // exercises the genuine QEMU/BIOS memory map, exactly like production does.
        BOOT_INFO_PTR.store(boot_info_raw, Ordering::Release);
    }

    pmm::init(false);
    interrupts::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: switching CR3 to a fully kernel-owned table survives in QEMU — the CPU
/// keeps fetching instructions correctly through the new table's higher-half mirror,
/// and serial I/O still works afterward.
/// Failure Impact: if this hangs or crashes even in QEMU (which tolerates far more than
/// real hardware — see the module doc), the mapping/switch logic itself is broken,
/// independent of the separate, real-hardware-only SMM risk.
#[test_case]
fn test_switch_to_direct_map_survives_in_qemu() {
    let old_pml4 = read_cr3() & 0x000F_FFFF_FFFF_F000;

    // SAFETY: the old firmware/BIOS-loader identity map is still active (no prior CR3
    // switch in this test kernel), and a real BootInfo was published in KernelMain.
    let new_pml4 = unsafe { direct_map::switch_to_direct_map(old_pml4) };

    assert_ne!(new_pml4, 0);
    assert_eq!(read_cr3() & 0x000F_FFFF_FFFF_F000, new_pml4);

    // If execution reaches here at all, the switch survived: this line, the assertions
    // above, and the test harness's own bookkeeping all execute from the higher-half
    // kernel mapping this new table had to preserve correctly.
    kaos_kernel::debugln!("Post-switch CR3 alive check OK, new_pml4={:#x}", new_pml4);
}
