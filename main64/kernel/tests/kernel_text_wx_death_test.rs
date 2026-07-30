//! Death test for #63 Phase 5: a ring-0 write to kernel `.text` must fault once the
//! kernel-owned page table (RO+X `.text`) is live and `CR0.WP` is set.
//!
//! This exercises the *production* W^X path end-to-end: it publishes the real
//! loader-provided `BootInfo` (magic-checked, exactly like `direct_map_full_switch_test`),
//! so `vmm::init` takes the kernel-owned-table switch (`switch_to_direct_map` →
//! `map_kernel_image_higher_half`, which maps this test binary's own `.text` RO+X), then
//! enables `CR0.WP` and writes through a pointer to a `.text` symbol. The resulting
//! ring-0 `#PF` (present + write) is turned into a `panic!("VMM: protection page fault …")`
//! by `page_fault.rs`, which the panic handler recognizes as success.
//!
//! Requirement: must be booted by a loader that publishes a `BootInfo` (the BIOS/UEFI
//! loaders do). On a BootInfo-less boot `vmm::init` falls back to the firmware clone
//! (RWX `.text`) and the write would silently succeed — the test would then fall through
//! to `exit_qemu(Failed)` rather than pass spuriously.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

use kaos_kernel::arch::{cpu, interrupts, msr};
use kaos_kernel::boot_info::BOOT_INFO_PTR;
use kaos_kernel::memory::{heap, pmm, vmm};

const BOOT_INFO_MAGIC: u64 = 0x4B41_4F53_5F42_4F4F; // "KAOS_BOO"

/// A stable `.text` symbol whose address we deliberately write to. `#[inline(never)]` +
/// `#[no_mangle]` keep it a real, addressable function in `.text` (RO+X after the switch).
#[no_mangle]
#[inline(never)]
extern "C" fn wx_probe_target() {
    core::hint::spin_loop();
}

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(boot_info_raw: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // SAFETY: mirrors main.rs's magic check before trusting the loader pointer; publishing
    // it is what makes `vmm::init` take the kernel-owned switch path.
    let has_boot_info =
        boot_info_raw != 0 && unsafe { *(boot_info_raw as *const u64) } == BOOT_INFO_MAGIC;
    if has_boot_info {
        BOOT_INFO_PTR.store(boot_info_raw, Ordering::Release);
    }

    msr::enable_no_execute();
    pmm::init(false);
    interrupts::init();
    vmm::init(false); // BootInfo published => switch to kernel-owned table; .text now RO+X
    heap::init(false);
    cpu::enable_write_protect(); // WP=1: RO .text is now enforced against ring 0

    test_main();

    // The test must panic (via the #PF) before reaching this point.
    kaos_kernel::arch::qemu::exit_qemu(kaos_kernel::arch::qemu::QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
    if kaos_kernel::testing::panic_message_contains(info, "VMM: protection page fault") {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: with the kernel-owned table live (RO+X `.text`) and `CR0.WP` set, a ring-0
/// write to a `.text` address faults with the protection-page-fault panic.
/// Failure Impact: if the write succeeds, kernel code is still writable at runtime — the
/// exact W^X hole Phase 5 closes. Release-blocking.
#[test_case]
fn test_write_to_ro_text_faults() {
    let target = wx_probe_target as *const () as *mut u8;
    // SAFETY: after the switch + WP, this `.text` page is read-only in ring 0, so the
    // write raises a #PF before any store takes effect (we never actually mutate code).
    unsafe {
        core::ptr::write_volatile(target, 0xCC);
    }
}
