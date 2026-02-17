//! GDT/TSS Integration Tests
//!
//! Validates the ring-3 foundation descriptors and TSS state wiring.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::gdt;

/// Entry point for the GDT/TSS test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: selector constants follow expected long-mode layout.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "selector constants follow expected long-mode layout".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_selector_constants() {
    assert_eq!(gdt::KERNEL_CODE_SELECTOR, 0x08);
    assert_eq!(gdt::KERNEL_DATA_SELECTOR, 0x10);
    assert_eq!(gdt::USER_CODE_SELECTOR, 0x1B);
    assert_eq!(gdt::USER_DATA_SELECTOR, 0x23);
    assert_eq!(gdt::TSS_SELECTOR, 0x28);
}

/// Contract: gdt init loads tss descriptor and kernel rsp0.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "gdt init loads tss descriptor and kernel rsp0".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_tss_descriptor_present_and_rsp0_nonzero() {
    assert!(gdt::is_initialized(), "GDT/TSS must be initialized");

    let descriptors = gdt::descriptor_snapshot();
    let tss_low = descriptors[5];
    let tss_high = descriptors[6];

    let tss_type = (tss_low >> 40) & 0x0F;
    let present = (tss_low >> 47) & 0x01;
    let base_low = ((tss_low >> 16) & 0xFFFF)
        | (((tss_low >> 32) & 0xFF) << 16)
        | (((tss_low >> 56) & 0xFF) << 24);
    let base_high = tss_high & 0xFFFF_FFFF;
    let base = base_low | (base_high << 32);

    assert!(
        tss_type == 0x9 || tss_type == 0xB,
        "TSS descriptor type must be available (0x9) or busy (0xB) 64-bit TSS"
    );
    assert_eq!(present, 1, "TSS descriptor must be marked present");
    assert_ne!(base, 0, "TSS base address must be non-zero");

    let rsp0 = gdt::kernel_rsp0();
    assert_ne!(rsp0, 0, "TSS RSP0 must be initialized");
}

/// Contract: kernel rsp0 setter updates tss state.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "kernel rsp0 setter updates tss state".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_set_kernel_rsp0_roundtrip() {
    let old_rsp0 = gdt::kernel_rsp0();
    let test_rsp0 = 0xFFFF_8000_0012_3000u64;

    gdt::set_kernel_rsp0(test_rsp0);
    assert_eq!(gdt::kernel_rsp0(), test_rsp0);

    gdt::set_kernel_rsp0(old_rsp0);
}

/// Contract: tss ist1 points to a dedicated aligned emergency stack.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "tss ist1 points to a dedicated aligned emergency stack".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_tss_ist1_is_initialized_and_aligned() {
    let ist1 = gdt::kernel_ist1();
    assert_ne!(ist1, 0, "TSS IST1 must be initialized");
    assert_eq!(ist1 & 0xF, 0, "TSS IST1 must be 16-byte aligned");
}
