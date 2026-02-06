//! Interrupt ABI/Layout Integration Tests
//!
//! Verifies the register-save layout used by IRQ trampolines and
//! basic PIT divisor calculations.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::mem::{size_of, MaybeUninit};
use core::panic::PanicInfo;
use core::ptr::addr_of;
use kaos_kernel::arch::interrupts::{self, TrapFrame};

/// Entry point for the interrupt-layout test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

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

#[test_case]
fn test_trap_frame_size_and_offsets() {
    assert!(
        size_of::<TrapFrame>() == 15 * 8,
        "TrapFrame must contain exactly 15 saved GPRs"
    );

    let tf = MaybeUninit::<TrapFrame>::uninit();
    let base = tf.as_ptr() as usize;

    // SAFETY:
    // - `addr_of!` does not dereference memory; it only computes field addresses.
    // - `tf` is `MaybeUninit`, which is valid for offset computations.
    unsafe {
        assert!((addr_of!((*tf.as_ptr()).r15) as usize) - base == 0);
        assert!((addr_of!((*tf.as_ptr()).r14) as usize) - base == 8);
        assert!((addr_of!((*tf.as_ptr()).r13) as usize) - base == 16);
        assert!((addr_of!((*tf.as_ptr()).r12) as usize) - base == 24);
        assert!((addr_of!((*tf.as_ptr()).r11) as usize) - base == 32);
        assert!((addr_of!((*tf.as_ptr()).r10) as usize) - base == 40);
        assert!((addr_of!((*tf.as_ptr()).r9) as usize) - base == 48);
        assert!((addr_of!((*tf.as_ptr()).r8) as usize) - base == 56);
        assert!((addr_of!((*tf.as_ptr()).rdi) as usize) - base == 64);
        assert!((addr_of!((*tf.as_ptr()).rsi) as usize) - base == 72);
        assert!((addr_of!((*tf.as_ptr()).rbp) as usize) - base == 80);
        assert!((addr_of!((*tf.as_ptr()).rbx) as usize) - base == 88);
        assert!((addr_of!((*tf.as_ptr()).rdx) as usize) - base == 96);
        assert!((addr_of!((*tf.as_ptr()).rcx) as usize) - base == 104);
        assert!((addr_of!((*tf.as_ptr()).rax) as usize) - base == 112);
    }
}

#[test_case]
fn test_irq_vector_constants_are_contiguous() {
    assert!(
        interrupts::IRQ1_VECTOR == interrupts::IRQ0_VECTOR + 1,
        "IRQ1 vector must follow IRQ0 vector"
    );
}

#[test_case]
fn test_exception_error_code_classification() {
    assert!(
        !interrupts::exception_has_error_code(interrupts::EXCEPTION_DIVIDE_ERROR),
        "divide error must not carry an error code"
    );
    assert!(
        !interrupts::exception_has_error_code(interrupts::EXCEPTION_INVALID_OPCODE),
        "invalid opcode must not carry an error code"
    );
    assert!(
        !interrupts::exception_has_error_code(interrupts::EXCEPTION_DEVICE_NOT_AVAILABLE),
        "device not available must not carry an error code"
    );
    assert!(
        interrupts::exception_has_error_code(interrupts::EXCEPTION_DOUBLE_FAULT),
        "double fault must carry an error code"
    );
    assert!(
        interrupts::exception_has_error_code(interrupts::EXCEPTION_GENERAL_PROTECTION),
        "general protection fault must carry an error code"
    );
    assert!(
        interrupts::exception_has_error_code(interrupts::EXCEPTION_PAGE_FAULT),
        "page fault must carry an error code"
    );
}

#[test_case]
fn test_pit_divisor_calculation() {
    assert!(interrupts::pit_divisor_for_hz(0) == 0);
    assert!(interrupts::pit_divisor_for_hz(1) == u16::MAX);
    assert!(interrupts::pit_divisor_for_hz(250) == 4772);
    assert!(interrupts::pit_divisor_for_hz(1000) == 1193);
    assert!(interrupts::pit_divisor_for_hz(2_000_000) == 1);
}
