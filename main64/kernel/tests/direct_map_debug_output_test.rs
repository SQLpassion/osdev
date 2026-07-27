//! Regression test: `vmm::init(true)` (debug output enabled) must not hang or panic.
//!
//! Every other integration test boots via `vmm::init(false)`, so a bug that only
//! triggers on the `debug_output = true` path — exactly production's `vmm::init(true)`
//! call in `main.rs` — went unnoticed by the whole test suite here. It was caught only
//! by a direct QEMU boot of the production kernel: the Phase 1 direct-map boot canary's
//! debug summary line originally called `vmm_logln`, which reads `VMM`'s shared state
//! via `with_vmm` — that `debug_assert!`s the VMM is already initialized, which isn't
//! true yet at the canary's call site (see `memory::vmm::direct_map::run_boot_canary`).
//! This test pins the fix (plain `debugln!`, no VMM-state dependency) by exercising the
//! exact call production makes.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{pmm, vmm};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(true); // debug_output = true, matching production main.rs.

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: `vmm::init(true)` completes (the boot canary's debug-output branch runs
/// without hitting the `with_vmm`/`debug_assert!` ordering bug) and leaves the VMM
/// usable afterward.
/// Failure Impact: a regression here reintroduces a debug-only production hang that no
/// other test in this suite would catch, since every other test boots with
/// `debug_output = false`.
#[test_case]
fn test_vmm_init_with_debug_output_completes_and_leaves_vmm_usable() {
    assert!(
        vmm::test_vmm(),
        "vmm::test_vmm() should succeed after a debug_output=true init"
    );
}
