//! Basic Boot Integration Test
//!
//! This test verifies that the kernel can boot and run basic operations.
//! It runs as a separate kernel binary in QEMU.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

/// Entry point for the integration test kernel
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    // Initialize serial for test output
    kaos_kernel::drivers::serial::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

// ============================================================================
// Integration Tests
// ============================================================================

#[test_case]
fn test_kernel_boots() {
    // If we get here, the kernel booted successfully!
}

#[test_case]
#[allow(clippy::eq_op)]
fn test_trivial_assertion() {
    assert_eq!(1 + 1, 2);
}

#[test_case]
fn test_vga_buffer_address() {
    // Verify the VGA buffer address is correct for higher-half kernel
    const VGA_BUFFER: usize = 0xFFFF8000000B8000;
    const { assert!(VGA_BUFFER > 0xFFFF800000000000, "VGA buffer should be in higher half") };
}
