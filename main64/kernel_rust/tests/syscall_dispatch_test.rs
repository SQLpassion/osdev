//! Syscall dispatcher integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::syscall::{self, is_valid_user_buffer, SysError, SyscallId};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: syscall ids remain stable.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "syscall ids remain stable".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_syscall_ids_are_stable() {
    assert!(SyscallId::Yield as u64 == 0, "Yield syscall id changed");
    assert!(SyscallId::WriteSerial as u64 == 1, "WriteSerial syscall id changed");
    assert!(SyscallId::Exit as u64 == 2, "Exit syscall id changed");
}

/// Contract: unknown syscall returns enosys.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "unknown syscall returns enosys".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_unknown_syscall_returns_enosys() {
    let ret = syscall::dispatch(0xDEAD, 1, 2, 3, 4);
    assert!(
        ret == syscall::SYSCALL_ERR_UNSUPPORTED,
        "unknown syscall must return ENOSYS"
    );
}

/// Contract: decode_result maps known syscall error values.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "decode_result maps known syscall error values".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_decode_result_maps_known_errors() {
    assert!(
        syscall::decode_result(syscall::SYSCALL_ERR_UNSUPPORTED) == Err(SysError::Enosys),
        "SYSCALL_ERR_UNSUPPORTED must decode to SysError::Enosys"
    );
    assert!(
        syscall::decode_result(syscall::SYSCALL_ERR_INVALID_ARG) == Err(SysError::Einval),
        "SYSCALL_ERR_INVALID_ARG must decode to SysError::Einval"
    );
}

/// Contract: decode_result keeps values below error sentinels as success.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "decode_result keeps values below error sentinels as success".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_decode_result_accepts_value_below_error_sentinels() {
    let raw = syscall::SYSCALL_ERR_INVALID_ARG - 1;
    assert!(
        syscall::decode_result(raw) == Ok(raw),
        "value below reserved error sentinels must remain a successful return"
    );
}

/// Contract: decode_result keeps successful values unchanged.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "decode_result keeps successful values unchanged".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_decode_result_passes_success_values() {
    assert!(syscall::decode_result(0) == Ok(0), "zero must remain a successful return");
    assert!(syscall::decode_result(17) == Ok(17), "positive result must remain unchanged");
}

/// Contract: write_serial rejects null pointer when len > 0.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "write_serial rejects null pointer when len > 0".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_write_serial_null_ptr_with_len_returns_einval() {
    let ret = syscall::dispatch(SyscallId::WriteSerial as u64, 0, 1, 0, 0);
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_serial with null pointer and len>0 must return EINVAL"
    );
}

/// Contract: write_serial with zero length is a no-op success.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "write_serial with zero length is a no-op success".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_write_serial_zero_len_returns_zero() {
    let ret = syscall::dispatch(SyscallId::WriteSerial as u64, 0, 0, 0, 0);
    assert!(ret == 0, "write_serial len=0 must return 0");
}

/// Contract: write_serial rejects kernel-space buffer pointer.
/// Given: A static buffer whose address lives in the kernel higher-half.
/// When: dispatch is called with that kernel-space pointer.
/// Then: The call must return EINVAL because user-pointer validation
///       rejects addresses outside user canonical space.
#[test_case]
fn test_write_serial_rejects_kernel_space_buffer() {
    static BYTES: &[u8] = b"ok";
    let ret = syscall::dispatch(
        SyscallId::WriteSerial as u64,
        BYTES.as_ptr() as u64,
        BYTES.len() as u64,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_serial with kernel-space buffer must return EINVAL"
    );
}

/// Contract: user alias rip preserves 4 KiB page offset.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "user alias rip preserves 4 KiB page offset".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_user_alias_rip_preserves_page_offset() {
    let alias = syscall::user_alias_rip(0x7000_0000_0000, 0xFFFF_8000_0012_3456);
    assert!(
        alias == 0x7000_0000_0456,
        "alias rip must keep original entry offset in mapped user code page"
    );
}

/// Contract: user alias va maps kernel higher-half offsets into user code window.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "user alias va maps kernel higher-half offsets into user code window".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_user_alias_va_for_kernel_maps_and_rejects_bounds() {
    let mapped = syscall::user_alias_va_for_kernel(
        0x7000_0000_0000,
        0x20_0000,
        0xFFFF_8000_0000_0000,
        0xFFFF_8000_0012_3000,
    );
    assert!(
        mapped == Some(0x7000_0000_0000 + 0x12_3000),
        "kernel VA offset must map into user code window"
    );

    let below_base = syscall::user_alias_va_for_kernel(
        0x7000_0000_0000,
        0x20_0000,
        0xFFFF_8000_0000_0000,
        0x7FFF_FFFF_FFFF_F000,
    );
    assert!(below_base.is_none(), "kernel VA below base must be rejected");

    let out_of_window = syscall::user_alias_va_for_kernel(
        0x7000_0000_0000,
        0x20_0000,
        0xFFFF_8000_0000_0000,
        0xFFFF_8000_0030_0000,
    );
    assert!(
        out_of_window.is_none(),
        "kernel VA beyond user code alias size must be rejected"
    );
}

// ── User-pointer validation ──────────────────────────────────────────

/// Contract: kernel-half pointer is rejected by user buffer validation.
#[test_case]
fn test_user_buffer_rejects_kernel_pointer() {
    let kernel_ptr = 0xFFFF_8000_0000_1000 as *const u8;
    assert!(
        !is_valid_user_buffer(kernel_ptr, 64),
        "kernel-half address must be rejected"
    );
}

/// Contract: pointer that crosses the canonical boundary is rejected.
#[test_case]
fn test_user_buffer_rejects_boundary_crossing() {
    // Start just below the user canonical limit, length pushes past it.
    let ptr = (0x0000_8000_0000_0000u64 - 16) as *const u8;
    assert!(
        !is_valid_user_buffer(ptr, 32),
        "buffer crossing canonical boundary must be rejected"
    );
}

/// Contract: arithmetic overflow in ptr+len is rejected.
#[test_case]
fn test_user_buffer_rejects_overflow() {
    let ptr = (u64::MAX - 1) as *const u8;
    assert!(
        !is_valid_user_buffer(ptr, 5),
        "ptr+len overflow must be rejected"
    );
}

/// Contract: null pointer with non-zero len is rejected.
#[test_case]
fn test_user_buffer_rejects_null_with_len() {
    assert!(
        !is_valid_user_buffer(core::ptr::null(), 1),
        "null pointer with len>0 must be rejected"
    );
}

/// Contract: zero-length buffer is always valid.
#[test_case]
fn test_user_buffer_accepts_zero_len() {
    // Even a kernel address is fine when len=0 (no access occurs).
    let kernel_ptr = 0xFFFF_8000_0000_0000 as *const u8;
    assert!(
        is_valid_user_buffer(kernel_ptr, 0),
        "zero-length buffer must always be valid"
    );
}

/// Contract: valid user-space pointer is accepted.
#[test_case]
fn test_user_buffer_accepts_valid_user_pointer() {
    let ptr = 0x0000_7000_0000_1000 as *const u8;
    assert!(
        is_valid_user_buffer(ptr, 256),
        "valid user-space pointer must be accepted"
    );
}

/// Contract: write_serial rejects kernel-half pointer via dispatch.
#[test_case]
fn test_write_serial_rejects_kernel_pointer() {
    let ret = syscall::dispatch(
        SyscallId::WriteSerial as u64,
        0xFFFF_8000_0000_1000, // kernel address
        8,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_serial with kernel pointer must return EINVAL"
    );
}
