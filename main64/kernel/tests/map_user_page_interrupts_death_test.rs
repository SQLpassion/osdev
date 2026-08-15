//! Death test for issue #51: `map_user_page`'s "run only inside `with_address_space`"
//! precondition must actually be checked, not just documented.
//!
//! `map_user_page`/`map_user_code_page` are safe `fn`s (not `unsafe fn`) that mutate
//! page tables through the recursive-mapping window. That window only resolves
//! correctly while `CR3` cannot change mid-call, which today is guaranteed solely by
//! convention: every sanctioned caller wraps the call in `with_address_space`, which
//! disables interrupts for its whole critical section. Nothing in the type system
//! stopped a future caller from invoking `map_user_page` directly with interrupts
//! still enabled.
//!
//! This test calls `vmm::map_user_page` directly, with interrupts deliberately left
//! enabled (PIC lines stay masked by `interrupts::init()`, so no IRQ actually fires),
//! and asserts that the `debug_assert!` added to the shared `map_user_leaf` body in
//! `kernel/src/memory/vmm/mapping.rs` catches the violation and panics before any
//! page-table write happens, rather than silently racing a future CR3 switch.
//!
//! See `kernel/src/memory/vmm/mapping.rs` (`map_user_leaf`).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use kaos_kernel::arch::interrupts;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::memory::vmm::USER_CODE_BASE;
use kaos_kernel::memory::{heap, pmm, vmm};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Mirror the standard VMM test boot sequence (see `vmm_test.rs`) so the
    // recursive-mapping window is live before we probe the guard.
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    test_main();

    // The test must panic (via the debug_assert) before reaching this point.
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let expected = "map_user_leaf: must run with interrupts disabled";
    if kaos_kernel::testing::panic_message_contains(info, expected) {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: calling `map_user_page` with interrupts enabled (i.e. outside
/// `with_address_space`) trips the debug-only precondition guard instead of
/// silently touching page tables under a CR3 that could change underneath it.
/// Failure Impact: without this guard, a future caller who forgets to wrap the
/// call in `with_address_space` gets no diagnostic — a preemption mid-call can
/// write into the wrong page-table hierarchy, corrupting an unrelated address
/// space. Release-blocking if the guard silently regresses.
#[test_case]
fn test_map_user_page_panics_when_interrupts_enabled() {
    // Interrupt lines are masked by `interrupts::init()` above, so enabling IF
    // here cannot actually deliver an IRQ mid-test; it only flips the flag the
    // guard inspects.
    interrupts::enable();

    // Any user-code-window address/PFN works: the debug_assert fires before
    // the function ever reads or writes a page-table entry.
    let _ = vmm::map_user_page(USER_CODE_BASE, 0, true);
}
