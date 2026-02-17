//! Process exec contract tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::memory::vmm;
use kaos_kernel::process;

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

/// Contract: process entry/stack/image constants stay aligned to VMM layout.
#[test_case]
fn test_process_constants_match_vmm_layout() {
    assert!(
        process::USER_PROGRAM_ENTRY_RIP == vmm::USER_CODE_BASE,
        "user entry rip must stay anchored at USER_CODE_BASE"
    );
    assert!(
        process::USER_PROGRAM_MAX_IMAGE_SIZE == vmm::USER_CODE_SIZE as usize,
        "max image size must match USER_CODE_SIZE window"
    );
    assert!(
        process::USER_PROGRAM_INITIAL_RSP
            == vmm::USER_STACK_TOP - process::USER_PROGRAM_STACK_ALIGNMENT,
        "initial rsp must be derived from USER_STACK_TOP and configured alignment"
    );
    assert!(
        process::USER_PROGRAM_INITIAL_RSP >= vmm::USER_STACK_BASE
            && process::USER_PROGRAM_INITIAL_RSP < vmm::USER_STACK_TOP,
        "initial rsp must lie within user stack mapping range"
    );
}

/// Contract: initial user stack pointer remains 16-byte aligned.
#[test_case]
fn test_initial_user_rsp_is_aligned() {
    assert!(
        process::USER_PROGRAM_INITIAL_RSP % process::USER_PROGRAM_STACK_ALIGNMENT == 0,
        "initial user rsp must stay aligned for ABI-compatible user code"
    );
}

/// Contract: image-size helper enforces configured user code bound.
#[test_case]
fn test_image_size_contract_helper() {
    assert!(
        process::image_fits_user_code(0),
        "zero-length image must be accepted by size contract"
    );
    assert!(
        process::image_fits_user_code(process::USER_PROGRAM_MAX_IMAGE_SIZE),
        "exact window-size image must be accepted by size contract"
    );
    assert!(
        !process::image_fits_user_code(process::USER_PROGRAM_MAX_IMAGE_SIZE + 1),
        "oversized image must be rejected by size contract"
    );
}

/// Contract: loaded-program descriptor preserves provided values.
#[test_case]
fn test_loaded_program_descriptor_roundtrip() {
    let descriptor = process::LoadedProgram::new(
        0x1234_5000,
        process::USER_PROGRAM_ENTRY_RIP,
        process::USER_PROGRAM_INITIAL_RSP,
        4096,
    );

    assert!(descriptor.cr3 == 0x1234_5000, "cr3 must be preserved");
    assert!(
        descriptor.entry_rip == process::USER_PROGRAM_ENTRY_RIP,
        "entry rip must be preserved"
    );
    assert!(
        descriptor.user_rsp == process::USER_PROGRAM_INITIAL_RSP,
        "user rsp must be preserved"
    );
    assert!(descriptor.image_len == 4096, "image length must be preserved");
}

/// Contract: ExecError equality remains discriminant-based and stable.
#[test_case]
fn test_exec_error_variant_distinction() {
    assert!(
        process::ExecError::InvalidName != process::ExecError::NotFound,
        "distinct exec failure causes must not collapse into one variant"
    );
    assert!(
        process::ExecError::Io == process::ExecError::Io,
        "same exec failure cause must compare equal"
    );
}
