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
use kaos_kernel::arch::interrupts::{self, SavedRegisters};
use kaos_kernel::syscall::SyscallId;

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

/// Contract: trap frame size and offsets.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "trap frame size and offsets".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_trap_frame_size_and_offsets() {
    assert!(
        size_of::<SavedRegisters>() == 15 * 8,
        "SavedRegisters must contain exactly 15 saved GPRs"
    );

    let tf = MaybeUninit::<SavedRegisters>::uninit();
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

/// Contract: irq vector constants are contiguous.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "irq vector constants are contiguous".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_irq_vector_constants_are_contiguous() {
    assert!(
        interrupts::IRQ1_KEYBOARD_VECTOR == interrupts::IRQ0_PIT_TIMER_VECTOR + 1,
        "IRQ1 vector must follow IRQ0 vector"
    );
}

/// Contract: exception vector constants match x86 spec.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "exception vector constants match x86 spec".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_exception_vector_constants_match_x86_spec() {
    assert!(
        interrupts::EXCEPTION_DIVIDE_ERROR == 0,
        "divide error vector must be 0"
    );
    assert!(
        interrupts::EXCEPTION_INVALID_OPCODE == 6,
        "invalid opcode vector must be 6"
    );
    assert!(
        interrupts::EXCEPTION_DEVICE_NOT_AVAILABLE == 7,
        "device-not-available vector must be 7"
    );
    assert!(
        interrupts::EXCEPTION_DOUBLE_FAULT == 8,
        "double-fault vector must be 8"
    );
    assert!(
        interrupts::EXCEPTION_GENERAL_PROTECTION == 13,
        "general-protection vector must be 13"
    );
    assert!(
        interrupts::EXCEPTION_PAGE_FAULT == 14,
        "page-fault vector must be 14"
    );
    assert!(
        interrupts::SYSCALL_INT80_VECTOR == 0x80,
        "syscall vector must be 0x80"
    );
}

/// Contract: exception error code classification.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "exception error code classification".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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

/// Contract: pit divisor calculation.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pit divisor calculation".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_pit_divisor_calculation() {
    assert!(interrupts::pit_divisor_for_hz(0) == 0);
    assert!(interrupts::pit_divisor_for_hz(1) == u16::MAX);
    assert!(interrupts::pit_divisor_for_hz(250) == 4772);
    assert!(interrupts::pit_divisor_for_hz(1000) == 1193);
    assert!(interrupts::pit_divisor_for_hz(2_000_000) == 1);
}

/// Contract: int 0x80 dispatches through static syscall table.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "int 0x80 dispatches through static syscall table".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_int80_syscall_dispatch_roundtrip() {
    interrupts::init();
    let mut ret_rax: u64 = SyscallId::WriteSerial as u64;
    // SAFETY:
    // - `interrupts::init()` loaded an IDT containing the `int 0x80` gate.
    // - The test executes in ring 0, so invoking software interrupt 0x80 is valid.
    // - Register constraints match the syscall ABI used by `syscall_rust_dispatch`.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inout("rax") ret_rax,
            in("rdi") 0u64,
            in("rsi") 0u64,
            in("rdx") 0u64,
            in("r10") 0u64,
        );
    }

    assert!(ret_rax == 0, "write_serial(len=0) syscall must return 0");
}
