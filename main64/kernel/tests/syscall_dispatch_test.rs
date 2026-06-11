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
    kaos_kernel::memory::pmm::init(false);
    kaos_kernel::memory::vmm::init(false);
    kaos_kernel::memory::heap::init(false);
    kaos_kernel::syscall::set_syscall_trace_enabled(false);
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: syscall trace logging toggle can be changed at runtime.
#[test_case]
fn test_syscall_trace_toggle_roundtrip() {
    let previous = syscall::syscall_trace_enabled();

    syscall::set_syscall_trace_enabled(false);
    assert!(
        !syscall::syscall_trace_enabled(),
        "trace toggle must report disabled after explicit disable"
    );

    syscall::set_syscall_trace_enabled(true);
    assert!(
        syscall::syscall_trace_enabled(),
        "trace toggle must report enabled after explicit enable"
    );

    syscall::set_syscall_trace_enabled(previous);
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
    assert!(
        SyscallId::WriteSerial as u64 == 1,
        "WriteSerial syscall id changed"
    );
    assert!(SyscallId::Exit as u64 == 2, "Exit syscall id changed");
    assert!(
        SyscallId::WriteConsole as u64 == 3,
        "WriteConsole syscall id changed"
    );
    assert!(SyscallId::GetChar as u64 == 4, "GetChar syscall id changed");
    assert!(SyscallId::GetCursor as u64 == 5, "GetCursor syscall id changed");
    assert!(SyscallId::SetCursor as u64 == 6, "SetCursor syscall id changed");
    assert!(
        SyscallId::ClearScreen as u64 == 7,
        "ClearScreen syscall id changed"
    );
    assert!(SyscallId::OpenFile as u64 == 8, "OpenFile syscall id changed");
    assert!(SyscallId::CloseFile as u64 == 9, "CloseFile syscall id changed");
    assert!(SyscallId::ReadFile as u64 == 10, "ReadFile syscall id changed");
    assert!(SyscallId::WriteFile as u64 == 11, "WriteFile syscall id changed");
    assert!(SyscallId::DeleteFile as u64 == 12, "DeleteFile syscall id changed");
    assert!(SyscallId::SeekFile as u64 == 13, "SeekFile syscall id changed");
    assert!(SyscallId::EndOfFile as u64 == 14, "EndOfFile syscall id changed");
    assert!(
        SyscallId::PrintRootDirectory as u64 == 15,
        "PrintRootDirectory syscall id changed"
    );
    assert!(SyscallId::Mmap as u64 == 16, "Mmap syscall id changed");
    assert!(SyscallId::Exec as u64 == 17, "Exec syscall id changed");
    assert!(SyscallId::Wait as u64 == 18, "Wait syscall id changed");
    assert!(SyscallId::Shutdown as u64 == 19, "Shutdown syscall id changed");
    assert!(
        SyscallId::WriteFramebuffer as u64 == 20,
        "WriteFramebuffer syscall id changed"
    );
    assert!(SyscallId::ReadKey as u64 == 21, "ReadKey syscall id changed");
    assert!(
        SyscallId::SetVgaMode as u64 == 22,
        "SetVgaMode syscall id changed"
    );
    assert!(
        SyscallId::GetPciDeviceCount as u64 == 23,
        "GetPciDeviceCount syscall id changed"
    );
    assert!(
        SyscallId::GetPciDevice as u64 == 24,
        "GetPciDevice syscall id changed"
    );
    assert!(
        SyscallId::GetBiosMemoryMapEntryCount as u64 == 25,
        "GetBiosMemoryMapEntryCount syscall id changed"
    );
    assert!(
        SyscallId::GetBiosMemoryMapEntry as u64 == 26,
        "GetBiosMemoryMapEntry syscall id changed"
    );
}

/// Contract: syscall number-to-name mapping for dispatcher logs stays stable.
#[test_case]
fn test_syscall_name_mapping_for_logging_is_stable() {
    assert!(
        syscall::syscall_name_for_number(SyscallId::Yield as u64) == "Yield",
        "Yield mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::WriteSerial as u64) == "WriteSerial",
        "WriteSerial mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::Exit as u64) == "Exit",
        "Exit mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::WriteConsole as u64) == "WriteConsole",
        "WriteConsole mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::GetChar as u64) == "GetChar",
        "GetChar mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::GetCursor as u64) == "GetCursor",
        "GetCursor mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::SetCursor as u64) == "SetCursor",
        "SetCursor mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::ClearScreen as u64) == "ClearScreen",
        "ClearScreen mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::OpenFile as u64) == "OpenFile",
        "OpenFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::CloseFile as u64) == "CloseFile",
        "CloseFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::ReadFile as u64) == "ReadFile",
        "ReadFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::WriteFile as u64) == "WriteFile",
        "WriteFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::DeleteFile as u64) == "DeleteFile",
        "DeleteFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::SeekFile as u64) == "SeekFile",
        "SeekFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::EndOfFile as u64) == "EndOfFile",
        "EndOfFile mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::PrintRootDirectory as u64) == "PrintRootDirectory",
        "PrintRootDirectory mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::Mmap as u64) == "Mmap",
        "Mmap mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::Exec as u64) == "Exec",
        "Exec mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::Wait as u64) == "Wait",
        "Wait mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::Shutdown as u64) == "Shutdown",
        "Shutdown mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::WriteFramebuffer as u64) == "WriteFramebuffer",
        "WriteFramebuffer mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::ReadKey as u64) == "ReadKey",
        "ReadKey mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::SetVgaMode as u64) == "SetVgaMode",
        "SetVgaMode mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::GetPciDeviceCount as u64) == "GetPciDeviceCount",
        "GetPciDeviceCount mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::GetPciDevice as u64) == "GetPciDevice",
        "GetPciDevice mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::GetBiosMemoryMapEntryCount as u64) == "GetBiosMemoryMapEntryCount",
        "GetBiosMemoryMapEntryCount mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(SyscallId::GetBiosMemoryMapEntry as u64) == "GetBiosMemoryMapEntry",
        "GetBiosMemoryMapEntry mapping must stay stable for syscall trace output"
    );
    assert!(
        syscall::syscall_name_for_number(0xDEAD) == "Unknown",
        "unknown syscall mapping must stay stable for syscall trace output"
    );
}

/// Contract: public WriteConsole syscall constant matches enum discriminant.
#[test_case]
fn test_write_console_constant_matches_enum_id() {
    assert!(
        SyscallId::WRITE_CONSOLE == SyscallId::WriteConsole as u64,
        "WRITE_CONSOLE constant must match WriteConsole enum value"
    );
}

/// Contract: public GetChar syscall constant matches enum discriminant.
#[test_case]
fn test_get_char_constant_matches_enum_id() {
    assert!(
        SyscallId::GET_CHAR == SyscallId::GetChar as u64,
        "GET_CHAR constant must match GetChar enum value"
    );
}

/// Contract: public GetCursor syscall constant matches enum discriminant.
#[test_case]
fn test_get_cursor_constant_matches_enum_id() {
    assert!(
        SyscallId::GET_CURSOR == SyscallId::GetCursor as u64,
        "GET_CURSOR constant must match GetCursor enum value"
    );
}

/// Contract: public SetCursor syscall constant matches enum discriminant.
#[test_case]
fn test_set_cursor_constant_matches_enum_id() {
    assert!(
        SyscallId::SET_CURSOR == SyscallId::SetCursor as u64,
        "SET_CURSOR constant must match SetCursor enum value"
    );
}

/// Contract: public ClearScreen syscall constant matches enum discriminant.
#[test_case]
fn test_clear_screen_constant_matches_enum_id() {
    assert!(
        SyscallId::CLEAR_SCREEN == SyscallId::ClearScreen as u64,
        "CLEAR_SCREEN constant must match ClearScreen enum value"
    );
}

/// Contract: GetChar syscall enum id remains stable.
#[test_case]
fn test_get_char_enum_id_is_stable() {
    assert!(SyscallId::GetChar as u64 == 4, "GetChar syscall id changed");
}

/// Contract: GetCursor syscall enum id remains stable.
#[test_case]
fn test_get_cursor_enum_id_is_stable() {
    assert!(SyscallId::GetCursor as u64 == 5, "GetCursor syscall id changed");
}

/// Contract: SetCursor syscall enum id remains stable.
#[test_case]
fn test_set_cursor_enum_id_is_stable() {
    assert!(SyscallId::SetCursor as u64 == 6, "SetCursor syscall id changed");
}

/// Contract: ClearScreen syscall enum id remains stable.
#[test_case]
fn test_clear_screen_enum_id_is_stable() {
    assert!(
        SyscallId::ClearScreen as u64 == 7,
        "ClearScreen syscall id changed"
    );
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
        syscall::decode_result(syscall::SYSCALL_ERR_UNSUPPORTED)
            == Err(SysError::UnsupportedSyscall),
        "SYSCALL_ERR_UNSUPPORTED must decode to SysError::UnsupportedSyscall"
    );
    assert!(
        syscall::decode_result(syscall::SYSCALL_ERR_INVALID_ARG) == Err(SysError::InvalidArgument),
        "SYSCALL_ERR_INVALID_ARG must decode to SysError::InvalidArgument"
    );
    assert!(
        syscall::decode_result(syscall::SYSCALL_ERR_IO) == Err(SysError::IoError),
        "SYSCALL_ERR_IO must decode to SysError::IoError"
    );
    assert!(
        syscall::decode_result(syscall::SYSCALL_ERR_OUT_OF_MEMORY) == Err(SysError::OutOfMemory),
        "SYSCALL_ERR_OUT_OF_MEMORY must decode to SysError::OutOfMemory"
    );
}

/// Contract: decode_result keeps values below error sentinels as success.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "decode_result keeps values below error sentinels as success".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_decode_result_accepts_value_below_error_sentinels() {
    let raw = syscall::SYSCALL_ERR_OUT_OF_MEMORY - 1;
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
    assert!(
        syscall::decode_result(0) == Ok(0),
        "zero must remain a successful return"
    );
    assert!(
        syscall::decode_result(17) == Ok(17),
        "positive result must remain unchanged"
    );
}

/// Contract: echo input normalization maps carriage return to newline.
#[test_case]
fn test_echo_input_normalization_maps_cr_to_lf() {
    let normalized = syscall::user::normalize_echo_input_byte(b'\r');
    assert!(
        normalized == b'\n',
        "carriage return must normalize to newline for one-shot echo"
    );
}

/// Contract: echo input normalization keeps newline unchanged.
#[test_case]
fn test_echo_input_normalization_keeps_lf() {
    let normalized = syscall::user::normalize_echo_input_byte(b'\n');
    assert!(
        normalized == b'\n',
        "newline must remain unchanged during echo normalization"
    );
}

/// Contract: echo input normalization preserves regular characters.
#[test_case]
fn test_echo_input_normalization_preserves_regular_bytes() {
    let normalized = syscall::user::normalize_echo_input_byte(b'A');
    assert!(
        normalized == b'A',
        "non-control bytes must remain unchanged during echo normalization"
    );
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
    assert!(
        below_base.is_none(),
        "kernel VA below base must be rejected"
    );

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

/// Contract: write_console rejects null pointer when len > 0.
#[test_case]
fn test_write_console_null_ptr_with_len_returns_einval() {
    let ret = syscall::dispatch(SyscallId::WriteConsole as u64, 0, 1, 0, 0);
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_console with null pointer and len>0 must return EINVAL"
    );
}

/// Contract: write_console with zero length is a no-op success.
#[test_case]
fn test_write_console_zero_len_returns_zero() {
    let ret = syscall::dispatch(SyscallId::WriteConsole as u64, 0, 0, 0, 0);
    assert!(ret == 0, "write_console len=0 must return 0");
}

/// Contract: write_console rejects kernel-space buffer pointer.
#[test_case]
fn test_write_console_rejects_kernel_pointer() {
    let ret = syscall::dispatch(
        SyscallId::WriteConsole as u64,
        0xFFFF_8000_0000_1000, // kernel address
        8,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_console with kernel pointer must return EINVAL"
    );
}

/// Contract: write_serial rejects (ptr, len) whose end overflows u64 — even after clamping
/// the len to MAX_SERIAL_WRITE_LEN would have produced a valid range.
///
/// A caller that passes usize::MAX as len is describing a structurally invalid buffer.
/// The kernel must reject the full claimed range, not silently accept a clamped subset.
#[test_case]
fn test_write_serial_rejects_overflowing_len() {
    // ptr is a valid user address; but ptr + usize::MAX overflows → EINVAL.
    let ret = syscall::dispatch(
        SyscallId::WriteSerial as u64,
        0x0000_0000_0001_0000, // valid user address
        usize::MAX as u64,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_serial with overflowing ptr+len must return EINVAL"
    );
}

/// Contract: write_serial rejects a (ptr, len) pair that crosses the user canonical boundary,
/// even though clamping len to MAX_SERIAL_WRITE_LEN would have kept the access inside user space.
///
/// The full claimed range must be valid, not just the first MAX bytes.
#[test_case]
fn test_write_serial_rejects_len_crossing_canonical_boundary() {
    // ptr is near the top of user space; ptr + 8193 crosses USER_CANONICAL_END.
    // After clamping to 4096, the access would be safe — but we must reject the
    // full claimed range before applying the DoS cap.
    let ptr = 0x0000_8000_0000_0000u64 - 4096; // last 4 KiB of user space
    let ret = syscall::dispatch(
        SyscallId::WriteSerial as u64,
        ptr,
        8193, // crosses the canonical boundary
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_serial with (ptr, len) crossing canonical boundary must return EINVAL"
    );
}

/// Contract: write_console rejects (ptr, len) whose end overflows u64.
#[test_case]
fn test_write_console_rejects_overflowing_len() {
    let ret = syscall::dispatch(
        SyscallId::WriteConsole as u64,
        0x0000_0000_0001_0000, // valid user address
        usize::MAX as u64,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_console with overflowing ptr+len must return EINVAL"
    );
}

/// Contract: write_console rejects a (ptr, len) pair that crosses the user canonical boundary.
#[test_case]
fn test_write_console_rejects_len_crossing_canonical_boundary() {
    let ptr = 0x0000_8000_0000_0000u64 - 4096;
    let ret = syscall::dispatch(
        SyscallId::WriteConsole as u64,
        ptr,
        8193,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_console with (ptr, len) crossing canonical boundary must return EINVAL"
    );
}

/// Contract: set_cursor + get_cursor roundtrip returns the requested position.
#[test_case]
fn test_set_cursor_then_get_cursor_roundtrip() {
    let ret = syscall::dispatch(SyscallId::SetCursor as u64, 7, 13, 0, 0);
    assert!(ret == 0, "set_cursor must return success");

    let packed = syscall::dispatch(SyscallId::GetCursor as u64, 0, 0, 0, 0);
    let row = (packed >> 32) as usize;
    let col = (packed & 0xFFFF_FFFF) as usize;
    assert!(row == 7, "get_cursor row must match previously set row");
    assert!(col == 13, "get_cursor col must match previously set col");
}

/// Contract: set_cursor clamps out-of-range coordinates to screen bounds.
#[test_case]
fn test_set_cursor_clamps_to_screen_bounds() {
    let ret = syscall::dispatch(
        SyscallId::SetCursor as u64,
        usize::MAX as u64,
        usize::MAX as u64,
        0,
        0,
    );
    assert!(ret == 0, "set_cursor must return success when clamping");

    let packed = syscall::dispatch(SyscallId::GetCursor as u64, 0, 0, 0, 0);
    let row = (packed >> 32) as usize;
    let col = (packed & 0xFFFF_FFFF) as usize;
    assert!(row == 24, "row must clamp to last VGA row");
    assert!(col == 79, "col must clamp to last VGA column");
}

/// Contract: clear_screen resets cursor to origin and returns success.
#[test_case]
fn test_clear_screen_resets_cursor_to_origin() {
    let set_ret = syscall::dispatch(SyscallId::SetCursor as u64, 12, 34, 0, 0);
    assert!(set_ret == 0, "set_cursor precondition must succeed");

    let clear_ret = syscall::dispatch(SyscallId::ClearScreen as u64, 0, 0, 0, 0);
    assert!(clear_ret == 0, "clear_screen must return success");

    let packed = syscall::dispatch(SyscallId::GetCursor as u64, 0, 0, 0, 0);
    let row = (packed >> 32) as usize;
    let col = (packed & 0xFFFF_FFFF) as usize;
    assert!(row == 0, "clear_screen must reset row to 0");
    assert!(col == 0, "clear_screen must reset col to 0");
}

/// Contract: exec with invalid user buffer returns invalid argument error.
#[test_case]
fn test_exec_with_invalid_pointer_returns_error() {
    let ret = syscall::dispatch(SyscallId::Exec as u64, 0, 0, 0, 0);
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "exec with null pointer must return invalid argument error"
    );
}

/// Contract: wait returns immediately for non-existent task.
#[test_case]
fn test_wait_for_invalid_task_returns_ok() {
    let ret = syscall::dispatch(SyscallId::Wait as u64, 9999, 0, 0, 0);
    assert!(
        ret == syscall::SYSCALL_OK,
        "wait for invalid task ID must return OK immediately"
    );
}

/// Contract: public WriteFramebuffer syscall constant matches enum discriminant.
#[test_case]
fn test_write_framebuffer_constant_matches_enum_id() {
    assert!(
        SyscallId::WRITE_FRAMEBUFFER == SyscallId::WriteFramebuffer as u64,
        "WRITE_FRAMEBUFFER constant must match WriteFramebuffer enum value"
    );
}

/// Contract: public ReadKey syscall constant matches enum discriminant.
#[test_case]
fn test_read_key_constant_matches_enum_id() {
    assert!(
        SyscallId::READ_KEY == SyscallId::ReadKey as u64,
        "READ_KEY constant must match ReadKey enum value"
    );
}

/// Contract: public SetVgaMode syscall constant matches enum discriminant.
#[test_case]
fn test_set_vga_mode_constant_matches_enum_id() {
    assert!(
        SyscallId::SET_VGA_MODE == SyscallId::SetVgaMode as u64,
        "SET_VGA_MODE constant must match SetVgaMode enum value"
    );
}

/// Contract: write_framebuffer rejects invalid lengths.
#[test_case]
fn test_write_framebuffer_rejects_invalid_len() {
    let buf = [0u16; 2000];
    let ret = syscall::dispatch(
        SyscallId::WriteFramebuffer as u64,
        buf.as_ptr() as u64,
        1999,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_framebuffer with len != 2000 must return EINVAL"
    );
}

/// Contract: write_framebuffer rejects misaligned pointers.
#[test_case]
fn test_write_framebuffer_rejects_misaligned_pointer() {
    let ret = syscall::dispatch(
        SyscallId::WriteFramebuffer as u64,
        0x1001,
        2000,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_framebuffer with misaligned ptr must return EINVAL"
    );
}

/// Contract: write_framebuffer rejects non-canonical or kernel pointers.
#[test_case]
fn test_write_framebuffer_rejects_non_canonical() {
    let ret = syscall::dispatch(
        SyscallId::WriteFramebuffer as u64,
        0xFFFF_8000_0000_0000u64,
        2000,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "write_framebuffer with non-canonical/kernel ptr must return EINVAL"
    );
}

/// Contract: write_framebuffer succeeds with a valid 80x25 buffer.
#[test_case]
fn test_write_framebuffer_success() {
    let phys = kaos_kernel::memory::vmm::page_table::alloc_frame_phys().unwrap();
    let pfn = kaos_kernel::memory::vmm::page_table::phys_to_pfn(phys);
    let user_va = kaos_kernel::memory::vmm::USER_CODE_BASE;

    kaos_kernel::memory::vmm::map_user_page(user_va, pfn, true).unwrap();

    let ptr = user_va as *mut u16;
    unsafe {
        // SAFETY:
        // - `user_va` was just mapped above and is valid and writable.
        // - Loop writes 2000 elements (4000 bytes), which is within the 4096-byte page size.
        for i in 0..2000 {
            ptr.add(i).write_volatile(0x0741);
        }
    }

    let ret = syscall::dispatch(
        SyscallId::WriteFramebuffer as u64,
        user_va,
        2000,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_OK,
        "write_framebuffer with valid buffer must return OK"
    );
}

/// Contract: set_vga_mode succeeds with valid flags.
#[test_case]
fn test_set_vga_mode_success() {
    let ret = syscall::dispatch(
        SyscallId::SetVgaMode as u64,
        0,
        0,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_OK,
        "set_vga_mode with 0 must return OK"
    );

    let ret = syscall::dispatch(
        SyscallId::SetVgaMode as u64,
        3,
        0,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_OK,
        "set_vga_mode with 3 must return OK"
    );
}

/// Contract: Exec system call clears the keyboard buffers even on invalid arguments.
#[test_case]
fn test_exec_clears_keyboard_buffers() {
    kaos_kernel::drivers::keyboard::init();

    // Enqueue 'a' make code (0x1e)
    kaos_kernel::drivers::keyboard::enqueue_raw_scancode(0x1e);
    assert!(
        kaos_kernel::drivers::keyboard::process_pending_scancodes(),
        "worker iteration should process queued scancode"
    );
    assert!(
        kaos_kernel::drivers::keyboard::read_char().is_some(),
        "precondition: character should be present in buffer"
    );

    // Re-populate buffer (since reading it consumed it)
    kaos_kernel::drivers::keyboard::enqueue_raw_scancode(0x1e);
    kaos_kernel::drivers::keyboard::process_pending_scancodes();

    // Invoke Exec syscall with invalid arguments (null pointer)
    let ret = syscall::dispatch(SyscallId::Exec as u64, 0, 0, 0, 0);
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "exec with null pointer must return invalid argument error"
    );

    // Verify keyboard buffers are now empty
    assert!(
        kaos_kernel::drivers::keyboard::read_char().is_none(),
        "exec syscall must clear legacy keyboard buffer"
    );
    assert!(
        kaos_kernel::drivers::keyboard::read_key().is_none(),
        "exec syscall must clear key event buffer"
    );
}

/// Contract: public PCI syscall constants match enum discriminants.
#[test_case]
fn test_pci_constants_match_enum_ids() {
    assert!(
        SyscallId::GET_PCI_DEVICE_COUNT == SyscallId::GetPciDeviceCount as u64,
        "GET_PCI_DEVICE_COUNT constant must match GetPciDeviceCount enum value"
    );
    assert!(
        SyscallId::GET_PCI_DEVICE == SyscallId::GetPciDevice as u64,
        "GET_PCI_DEVICE constant must match GetPciDevice enum value"
    );
}

/// Contract: PCI query syscalls correctly report device count and copy metadata to user buffers.
#[test_case]
fn test_pci_query_syscalls() {
    kaos_kernel::drivers::pci::init();

    let count_ret = syscall::dispatch(
        SyscallId::GetPciDeviceCount as u64,
        0,
        0,
        0,
        0,
    );
    assert!(
        count_ret != syscall::SYSCALL_ERR_UNSUPPORTED,
        "GetPciDeviceCount must be supported"
    );

    let count = count_ret as usize;

    let invalid_idx = count as u64;
    let ret = syscall::dispatch(
        SyscallId::GetPciDevice as u64,
        invalid_idx,
        0,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "querying out-of-bounds pci index must return invalid argument"
    );

    if count > 0 {
        let phys = kaos_kernel::memory::vmm::page_table::alloc_frame_phys().unwrap();
        let pfn = kaos_kernel::memory::vmm::page_table::phys_to_pfn(phys);
        let user_va = kaos_kernel::memory::vmm::USER_CODE_BASE + 0x1000;

        kaos_kernel::memory::vmm::map_user_page(user_va, pfn, true).unwrap();

        let ret = syscall::dispatch(
            SyscallId::GetPciDevice as u64,
            0,
            user_va,
            0,
            0,
        );
        assert!(
            ret == syscall::SYSCALL_OK,
            "querying index 0 with valid user buffer must return OK"
        );

        let user_dev = user_va as *const kaos_kernel::syscall::UserPciDevice;
        // SAFETY:
        // - `user_va` has been mapped to physical memory and populated by the kernel.
        // - We hold read-only access to this memory block.
        let bus = unsafe { (*user_dev).bus };
        let kernel_devs = kaos_kernel::drivers::pci::get_devices();
        assert!(
            bus == kernel_devs[0].bus,
            "user-copied device bus must match kernel-scanned device bus"
        );
    }
}

/// Contract: public BIOS syscall constants match enum discriminants.
#[test_case]
fn test_bios_constants_match_enum_ids() {
    assert!(
        SyscallId::GET_BIOS_MEMORY_MAP_ENTRY_COUNT == SyscallId::GetBiosMemoryMapEntryCount as u64,
        "GET_BIOS_MEMORY_MAP_ENTRY_COUNT constant must match GetBiosMemoryMapEntryCount enum value"
    );
    assert!(
        SyscallId::GET_BIOS_MEMORY_MAP_ENTRY == SyscallId::GetBiosMemoryMapEntry as u64,
        "GET_BIOS_MEMORY_MAP_ENTRY constant must match GetBiosMemoryMapEntry enum value"
    );
}

/// Contract: BIOS query syscalls correctly report entry count and copy metadata to user buffers.
#[test_case]
fn test_bios_memory_map_query_syscalls() {
    let count_ret = syscall::dispatch(
        SyscallId::GetBiosMemoryMapEntryCount as u64,
        0,
        0,
        0,
        0,
    );
    assert!(
        count_ret != syscall::SYSCALL_ERR_UNSUPPORTED,
        "GetBiosMemoryMapEntryCount must be supported"
    );

    let count = count_ret as usize;

    let invalid_idx = count as u64;
    let ret = syscall::dispatch(
        SyscallId::GetBiosMemoryMapEntry as u64,
        invalid_idx,
        0,
        0,
        0,
    );
    assert!(
        ret == syscall::SYSCALL_ERR_INVALID_ARG,
        "querying out-of-bounds BIOS memory map index must return invalid argument"
    );

    if count > 0 {
        let phys = kaos_kernel::memory::vmm::page_table::alloc_frame_phys().unwrap();
        let pfn = kaos_kernel::memory::vmm::page_table::phys_to_pfn(phys);
        let user_va = kaos_kernel::memory::vmm::USER_CODE_BASE + 0x2000;

        kaos_kernel::memory::vmm::map_user_page(user_va, pfn, true).unwrap();

        let ret = syscall::dispatch(
            SyscallId::GetBiosMemoryMapEntry as u64,
            0,
            user_va,
            0,
            0,
        );
        assert!(
            ret == syscall::SYSCALL_OK,
            "querying index 0 with valid user buffer must return OK"
        );

        let user_region = user_va as *const kaos_kernel::syscall::UserBiosMemoryRegion;
        // SAFETY:
        // - `user_va` has been mapped to physical memory and populated by the kernel.
        // - We hold read-only access to this memory block.
        let size = unsafe { (*user_region).size };
        let region = kaos_kernel::memory::bios::MEMORYMAP_OFFSET as *const kaos_kernel::memory::bios::BiosMemoryRegion;
        let kernel_entry = unsafe { &*region };
        assert!(
            size == kernel_entry.size,
            "user-copied region size must match kernel BIOS region size"
        );
    }
}


