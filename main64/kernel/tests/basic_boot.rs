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

/// Contract: kernel boots.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "kernel boots".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_kernel_boots() {
    // If we get here, the kernel booted successfully!
}

#[test_case]
#[allow(clippy::eq_op)]
/// Contract: trivial arithmetic assertion.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "trivial arithmetic assertion".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
fn test_trivial_assertion() {
    assert_eq!(1 + 1, 2);
}

/// Contract: vga buffer address.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "vga buffer address".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_vga_buffer_address() {
    // Verify the VGA buffer address is correct for higher-half kernel
    const VGA_BUFFER: usize = 0xFFFF8000000B8000;
    const {
        assert!(
            VGA_BUFFER > 0xFFFF800000000000,
            "VGA buffer should be in higher half"
        )
    };
}

#[test_case]
/// Contract: boot info parsing.
/// Given: A manually constructed BootInfo structure in memory.
/// When: We check its magic number and parse it.
/// Then: The structure must be recognized as valid, and its values must match the expected fields.
fn test_boot_info_parsing() {
    use kaos_kernel::boot_info::{BootInfo, VideoModeType, FramebufferInfo, UnifiedMemoryEntry};

    static mut DUMMY_MEM_MAP: [UnifiedMemoryEntry; 2] = [
        UnifiedMemoryEntry { start: 0x1000, size: 0x1000, is_usable: true },
        UnifiedMemoryEntry { start: 0x2000, size: 0x2000, is_usable: false },
    ];

    let info = BootInfo {
        magic: 0x4B414F535F424F4F,
        video_type: VideoModeType::VgaText,
        fb_info: FramebufferInfo {
            base_address: 0,
            size: 0,
            width: 0,
            height: 0,
            pixels_per_scanline: 0,
        },
        memory_map_addr: unsafe { &raw const DUMMY_MEM_MAP[0] as u64 },
        memory_map_len: 2,
        kernel_size: 123456,
    };

    let raw_ptr = &info as *const BootInfo as u64;

    assert!(raw_ptr > 0x1000);
    assert_eq!(raw_ptr % 8, 0);

    let parsed_magic = unsafe { *(raw_ptr as *const u64) };
    assert_eq!(parsed_magic, 0x4B414F535F424F4F);

    let parsed_info = unsafe { &*(raw_ptr as *const BootInfo) };
    assert_eq!(parsed_info.kernel_size, 123456);
    assert_eq!(parsed_info.memory_map_len, 2);
    assert_eq!(parsed_info.video_type, VideoModeType::VgaText);
}
