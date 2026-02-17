//! Process exec contract tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use core::panic::PanicInfo;
use kaos_kernel::arch::{gdt, interrupts};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process;
use kaos_kernel::scheduler;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);
    kaos_kernel::drivers::ata::init();
    kaos_kernel::io::fat12::init();
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

/// Contract: FAT12 loader returns the bundled user program and validates size bounds.
#[test_case]
fn test_load_program_image_reads_hello_bin() {
    kaos_kernel::drivers::ata::init();

    let image = process::load_program_image("hello.bin")
        .expect("hello.bin must be loadable via process FAT12 loader");
    assert!(
        !image.is_empty(),
        "loaded user image must contain bytes"
    );
    assert!(
        image.len() <= process::USER_PROGRAM_MAX_IMAGE_SIZE,
        "loaded user image must fit configured executable mapping window"
    );
}

/// Contract: loader maps FAT12 invalid-name failures to `ExecError::InvalidName`.
#[test_case]
fn test_load_program_image_maps_invalid_name_error() {
    let result = process::load_program_image("invalid.name.txt");
    assert!(
        matches!(result, Err(process::ExecError::InvalidName)),
        "invalid FAT short name must map to ExecError::InvalidName"
    );
}

/// Contract: loader maps FAT12 missing-file failures to `ExecError::NotFound`.
#[test_case]
fn test_load_program_image_maps_not_found_error() {
    kaos_kernel::drivers::ata::init();

    let result = process::load_program_image("missing.bin");
    assert!(
        matches!(result, Err(process::ExecError::NotFound)),
        "missing FAT12 entry must map to ExecError::NotFound"
    );
}

/// Contract: explicit image-length validator enforces non-empty lower bound and
/// user code window upper bound.
#[test_case]
fn test_validate_program_image_len_enforces_upper_bound() {
    assert!(
        matches!(
            process::validate_program_image_len(0),
            Err(process::ExecError::FileTooLarge)
        ),
        "zero-length image must be rejected by loader size validator"
    );
    assert!(
        process::validate_program_image_len(1).is_ok(),
        "single-byte image must be accepted by loader size validator"
    );
    assert!(
        process::validate_program_image_len(process::USER_PROGRAM_MAX_IMAGE_SIZE).is_ok(),
        "exact limit image must be accepted by loader size validator"
    );
    assert!(
        matches!(
            process::validate_program_image_len(process::USER_PROGRAM_MAX_IMAGE_SIZE + 1),
            Err(process::ExecError::FileTooLarge)
        ),
        "oversized image must be rejected by loader size validator"
    );
}

/// Contract: image mapper creates dedicated user mappings and copies bytes.
#[test_case]
fn test_map_program_image_into_user_address_space_maps_copy_and_permissions() {
    let image = process::load_program_image("hello.bin")
        .expect("hello.bin must be loadable before map/copy integration step");
    let loaded = process::map_program_image_into_user_address_space(&image)
        .expect("valid user image must map/copy into fresh user CR3");

    assert!(loaded.cr3 != 0, "mapped program must return non-zero CR3");
    assert!(
        loaded.entry_rip == process::USER_PROGRAM_ENTRY_RIP,
        "mapped program entry rip must match process contract"
    );
    assert!(
        loaded.user_rsp == process::USER_PROGRAM_INITIAL_RSP,
        "mapped program rsp must match process contract"
    );
    assert!(
        loaded.image_len == image.len(),
        "mapped program descriptor must preserve source image length"
    );

    let code_page_count = if loaded.image_len == 0 {
        0
    } else {
        (loaded.image_len + pmm::PAGE_SIZE as usize - 1) / pmm::PAGE_SIZE as usize
    };

    let mut code_pfns = Vec::with_capacity(code_page_count);
    let mut stack_pfn = 0u64;
    vmm::with_address_space(loaded.cr3, || {
        for idx in 0..code_page_count {
            let code_page_va = process::USER_PROGRAM_ENTRY_RIP + idx as u64 * pmm::PAGE_SIZE;
            let code_flags = vmm::debug_mapping_flags_for_va(code_page_va)
                .expect("mapped code page must expose mapping flags");
            assert!(
                code_flags == (true, true, true, true, false),
                "code page must be user-accessible and read-only after copy"
            );

            let mapped_pfn = vmm::debug_mapped_pfn_for_va(code_page_va)
                .expect("mapped code page must expose leaf PFN");
            code_pfns.push(mapped_pfn);
        }

        let stack_page_va = vmm::USER_STACK_TOP - pmm::PAGE_SIZE;
        let stack_flags = vmm::debug_mapping_flags_for_va(stack_page_va)
            .expect("mapped bootstrap stack page must expose mapping flags");
        assert!(
            stack_flags == (true, true, true, true, true),
            "bootstrap stack page must be user-accessible and writable"
        );
        stack_pfn = vmm::debug_mapped_pfn_for_va(stack_page_va)
            .expect("mapped bootstrap stack page must expose leaf PFN");

        // SAFETY:
        // - Loader mapped code pages in this address space and copied `image` bytes.
        // - Reading `image.len()` bytes from `USER_PROGRAM_ENTRY_RIP` is valid.
        unsafe {
            let code_base = process::USER_PROGRAM_ENTRY_RIP as *const u8;
            for (idx, expected) in image.iter().enumerate() {
                let actual = core::ptr::read_volatile(code_base.add(idx));
                assert!(
                    actual == *expected,
                    "mapped image byte mismatch at offset {}: expected 0x{:02x}, got 0x{:02x}",
                    idx,
                    *expected,
                    actual
                );
            }
        }
    });

    vmm::destroy_user_address_space(loaded.cr3);

    // Release code PFNs explicitly because current VMM teardown keeps USER_CODE
    // leaf PFNs reserved to support temporary kernel-text alias mappings.
    pmm::with_pmm(|mgr| {
        for pfn in code_pfns {
            let _ = mgr.release_pfn(pfn);
        }
        let _ = mgr.release_pfn(stack_pfn);
    });
}

extern "C" fn parked_kernel_task() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Contract: `exec_from_fat12` maps image and spawns a scheduler user task.
#[test_case]
fn test_exec_from_fat12_spawns_user_task() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id =
        process::exec_from_fat12("hello.bin").expect("hello.bin exec path must spawn user task");

    assert!(
        scheduler::is_user_task(task_id),
        "exec_from_fat12 must create a user-mode scheduler entry"
    );

    let (task_cr3, user_rsp, _kernel_rsp_top) = scheduler::task_context(task_id)
        .expect("spawned user task must expose scheduler context tuple");
    assert!(task_cr3 != 0, "spawned user task CR3 must be non-zero");
    assert!(
        user_rsp == process::USER_PROGRAM_INITIAL_RSP,
        "spawned user task must preserve configured initial user RSP"
    );

    let iret_frame = scheduler::task_iret_frame(task_id)
        .expect("spawned user task must expose initial iret frame");
    assert!(
        iret_frame.rip == process::USER_PROGRAM_ENTRY_RIP,
        "spawned user task RIP must point to configured user entry base"
    );
    assert!(
        iret_frame.rsp == process::USER_PROGRAM_INITIAL_RSP,
        "spawned user task IRET rsp must match process user rsp contract"
    );

    assert!(
        scheduler::terminate_task(task_id),
        "spawned user task must be terminatable for test cleanup"
    );
}

/// Contract: terminating an exec-loaded task releases loader-owned code PFNs.
#[test_case]
fn test_exec_from_fat12_terminate_releases_loader_code_pfn() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id =
        process::exec_from_fat12("hello.bin").expect("hello.bin exec path must spawn user task");
    let (task_cr3, _, _) = scheduler::task_context(task_id)
        .expect("spawned user task must expose scheduler context tuple");

    let code_pfn = vmm::with_address_space(task_cr3, || {
        vmm::debug_mapped_pfn_for_va(process::USER_PROGRAM_ENTRY_RIP)
            .expect("exec-loaded task must map first user code page")
    });

    assert!(
        scheduler::terminate_task(task_id),
        "spawned user task must be terminatable for test cleanup"
    );

    let released_again = pmm::with_pmm(|mgr| mgr.release_pfn(code_pfn));
    assert!(
        !released_again,
        "loader-owned code PFN must already be released during task teardown"
    );
}

/// Contract: `exec_from_fat12` maps scheduler spawn errors to `ExecError::SpawnFailed`.
#[test_case]
fn test_exec_from_fat12_maps_spawn_failure_to_spawn_failed() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let mut filler_task_ids = Vec::new();
    loop {
        match scheduler::spawn_kernel_task(parked_kernel_task) {
            Ok(task_id) => filler_task_ids.push(task_id),
            Err(_) => break,
        }
    }

    assert!(
        !filler_task_ids.is_empty(),
        "test precondition: scheduler must accept at least one filler task before capacity is hit"
    );

    let result = process::exec_from_fat12("hello.bin");
    assert!(
        matches!(result, Err(process::ExecError::SpawnFailed)),
        "scheduler-capacity spawn failure must map to ExecError::SpawnFailed"
    );

    for task_id in filler_task_ids {
        assert!(
            scheduler::terminate_task(task_id),
            "filler task must be terminatable for test cleanup"
        );
    }
}

/// Contract: `exec_from_fat12` maps invalid FAT12 short names to `ExecError::InvalidName`.
#[test_case]
fn test_exec_from_fat12_maps_invalid_name_error() {
    let result = process::exec_from_fat12("invalid.name.txt");
    assert!(
        matches!(result, Err(process::ExecError::InvalidName)),
        "invalid 8.3 input must fail early with ExecError::InvalidName"
    );
}
