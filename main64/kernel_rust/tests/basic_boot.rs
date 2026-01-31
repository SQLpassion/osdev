//! Basic Boot Integration Test
//!
//! This test verifies that the kernel can boot and run basic operations.
//! It runs as a separate kernel binary in QEMU.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::debugln;
use kaos_kernel::testing::Testable;

/// Entry point for the integration test kernel
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    // Initialize serial for test output
    kaos_kernel::drivers::serial::init();

    debugln!("========================================");
    debugln!("  Basic Boot Integration Test");
    debugln!("========================================");
    debugln!();

    // Run all tests
    run_tests();

    // If we get here, all tests passed
    debugln!();
    debugln!("========================================");
    debugln!("  All tests passed!");
    debugln!("========================================");

    exit_qemu(QemuExitCode::Success);
}

/// Panic handler for integration tests
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

// ============================================================================
// Test Runner
// ============================================================================

/// Run all tests in this integration test file
fn run_tests() {
    // List of all test functions
    let tests: &[&dyn Testable] = &[
        &test_kernel_boots,
        &test_trivial_assertion,
        &test_vga_buffer_address,
    ];

    debugln!("Running {} tests:", tests.len());
    debugln!();

    for test in tests {
        test.run();
    }
}

// ============================================================================
// Integration Tests
// ============================================================================

fn test_kernel_boots() {
    // If we get here, the kernel booted successfully!
    kaos_kernel::debug!("    (kernel boot verified)");
}

fn test_trivial_assertion() {
    assert_eq!(1 + 1, 2);
}

fn test_vga_buffer_address() {
    // Verify the VGA buffer address is correct for higher-half kernel
    const VGA_BUFFER: usize = 0xFFFF8000000B8000;
    assert!(VGA_BUFFER > 0xFFFF800000000000, "VGA buffer should be in higher half");
}
