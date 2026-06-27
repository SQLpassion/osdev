//! PMM metadata-base selection — focused unit test (pure; no firmware, no allocation).
//!
//! `PhysicalMemoryManager::new()` must place its layout (header + region array + bitmaps)
//! in the bootloader-reserved region (`BootInfo.pmm_metadata_base`) when one is provided,
//! and otherwise fall back to "right after the kernel image/BSS". On large-RAM UEFI
//! systems the bitmaps are far too big to sit in low memory, so picking the wrong base
//! triple-faulted real hardware (see `docs/pmm.md` §2). That decision is factored into the
//! pure helper `select_metadata_base`, which this test pins directly — no BootInfo pointer,
//! no `__bss_end`, no side effects.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use kaos_kernel::memory::pmm::manager::select_metadata_base;

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
// Tests
// ============================================================================

/// A stand-in "address right after the kernel BSS" for the fallback cases.
const KERNEL_END_PHYS: u64 = 0x0020_0000;
/// A stand-in bootloader-reserved metadata base, far above low memory (UEFI-style).
const RESERVED_BASE: u64 = 0x0000_0020_0000_0000;

/// Contract: with a non-zero bootloader-reserved base, the PMM uses that base (UEFI path).
/// Failure Impact: the bitmaps would land in low memory and overrun firmware on large-RAM
///        hardware — the original triple-fault. Release-blocking.
#[test_case]
fn test_uses_reserved_base_when_present() {
    assert_eq!(
        select_metadata_base(Some(RESERVED_BASE), KERNEL_END_PHYS),
        RESERVED_BASE,
        "a non-zero pmm_metadata_base must win over the kernel-end fallback"
    );
}

/// Contract: with no BootInfo (BIOS loader / tests), the PMM falls back to the kernel end.
/// Failure Impact: the BIOS path would dereference a bogus base. Release-blocking.
#[test_case]
fn test_falls_back_when_no_boot_info() {
    assert_eq!(
        select_metadata_base(None, KERNEL_END_PHYS),
        KERNEL_END_PHYS,
        "absent BootInfo must fall back to the address after the kernel image"
    );
}

/// Contract: a BootInfo present but with `pmm_metadata_base == 0` also falls back.
/// (`0` is the loader's "no reserved region" sentinel — see `BootInfo` docs.)
/// Failure Impact: treating 0 as a real base would point the layout at the null page.
///        Release-blocking.
#[test_case]
fn test_falls_back_when_reserved_base_zero() {
    assert_eq!(
        select_metadata_base(Some(0), KERNEL_END_PHYS),
        KERNEL_END_PHYS,
        "a zero pmm_metadata_base is the 'not provided' sentinel and must fall back"
    );
}
