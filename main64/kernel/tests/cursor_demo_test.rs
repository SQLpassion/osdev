//! Cursor demo end-to-end regression test.
//!
//! This test executes the real `cursordemo` ring-3 flow and verifies that it
//! returns cleanly instead of faulting on missing user-code mappings.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use kaos_kernel::arch::{gdt, interrupts};
use kaos_kernel::memory::{heap, pmm, vmm};

// Re-export kernel modules under this test crate root so the included
// `cursor_demo.rs` can use its original `crate::...` paths unchanged.
mod drivers {
    pub use kaos_kernel::drivers::*;
}

mod memory {
    pub use kaos_kernel::memory::*;
}

mod scheduler {
    pub use kaos_kernel::scheduler::*;
}

mod syscall {
    pub use kaos_kernel::syscall::*;
}

/// Kernel higher-half base used to translate symbol VAs to physical offsets.
const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_8000_0000_0000;

#[inline]
fn kernel_va_to_phys(kernel_va: u64) -> Option<u64> {
    if kernel_va >= KERNEL_HIGHER_HALF_BASE {
        Some(kernel_va - KERNEL_HIGHER_HALF_BASE)
    } else {
        None
    }
}

#[inline]
fn kernel_va_to_user_code_va(kernel_va: u64) -> Option<u64> {
    syscall::user_alias_va_for_kernel(
        vmm::USER_CODE_BASE,
        vmm::USER_CODE_SIZE,
        KERNEL_HIGHER_HALF_BASE,
        kernel_va,
    )
}

#[path = "../src/user_tasks/cursor_demo.rs"]
mod cursor_demo_impl;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    drivers::serial::init();
    syscall::set_syscall_trace_enabled(false);
    gdt::init();
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: ring-3 cursor demo runs to completion without instruction-fetch faults.
#[test_case]
fn test_cursor_demo_runs_to_completion() {
    // Step 1: Preload one ESC key into decoded keyboard input so the demo exits
    // deterministically from its final getchar loop.
    drivers::keyboard::init();
    drivers::keyboard::enqueue_raw_scancode(0x01);
    assert!(
        drivers::keyboard::process_pending_scancodes(),
        "precondition: injected ESC scancode must be decoded"
    );

    // Step 2: Start scheduler and bind current kernel CR3 as shared kernel root.
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    // Step 3: Run the full cursor demo path (map code/data pages, spawn ring-3 task,
    // wait for completion). The test fails if this path triggers a fatal fault.
    cursor_demo_impl::run_user_mode_cursor_demo();

    // Step 4: ESC should have been consumed by the ring-3 task.
    assert!(
        drivers::keyboard::read_char().is_none(),
        "cursor demo should consume injected ESC input"
    );
}
