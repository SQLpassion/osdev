//! Process exec contract tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts::SavedRegisters;
use kaos_kernel::arch::{gdt, interrupts};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process;
use kaos_kernel::scheduler::{self, TaskState};
use kaos_kernel::syscall::{self, SyscallId};

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
    kaos_kernel::drivers::block::init_ata();
    let vol = kaos_kernel::io::fat32::Fat32Volume::mount(0)
        .expect("FAT32 superfloppy must mount at LBA 0 in the test image");
    kaos_kernel::io::vfs::mount(alloc::boxed::Box::new(
        kaos_kernel::io::fat32::Fat32Fs::new(vol),
    ));
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
    let entry_rip = core::hint::black_box(process::USER_PROGRAM_ENTRY_RIP);
    let user_code_base = core::hint::black_box(vmm::USER_CODE_BASE);
    assert!(
        entry_rip == user_code_base,
        "user entry rip must stay anchored at USER_CODE_BASE"
    );

    let max_image_size = core::hint::black_box(process::USER_PROGRAM_MAX_IMAGE_SIZE);
    let user_code_size = core::hint::black_box(vmm::USER_CODE_SIZE as usize);
    assert!(
        max_image_size == user_code_size,
        "max image size must match USER_CODE_SIZE window"
    );

    let initial_rsp = core::hint::black_box(process::USER_PROGRAM_INITIAL_RSP);
    let expected_rsp =
        core::hint::black_box(vmm::USER_STACK_TOP - process::USER_PROGRAM_STACK_ALIGNMENT);
    assert!(
        initial_rsp == expected_rsp,
        "initial rsp must be derived from USER_STACK_TOP and configured alignment"
    );

    let stack_base = core::hint::black_box(vmm::USER_STACK_BASE);
    let stack_top = core::hint::black_box(vmm::USER_STACK_TOP);
    assert!(
        initial_rsp >= stack_base && initial_rsp < stack_top,
        "initial rsp must lie within user stack mapping range"
    );
}

/// Contract: initial user stack pointer remains 16-byte aligned.
#[test_case]
fn test_initial_user_rsp_is_aligned() {
    let initial_rsp = core::hint::black_box(process::USER_PROGRAM_INITIAL_RSP);
    let alignment = core::hint::black_box(process::USER_PROGRAM_STACK_ALIGNMENT);
    assert!(
        initial_rsp.is_multiple_of(alignment),
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
        1,
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
    assert!(
        descriptor.image_len == 4096,
        "image length must be preserved"
    );
    assert!(
        descriptor.code_page_count == 1,
        "code page count must be preserved"
    );
}

/// Contract: ExecError equality remains discriminant-based and stable.
#[test_case]
fn test_exec_error_variant_distinction() {
    assert!(
        process::ExecError::InvalidName != process::ExecError::NotFound,
        "distinct exec failure causes must not collapse into one variant"
    );
    let io_err = core::hint::black_box(process::ExecError::Io);
    assert!(
        io_err == process::ExecError::Io,
        "same exec failure cause must compare equal"
    );
    assert!(
        process::ExecError::OutOfMemory != process::ExecError::MappingFailed,
        "OOM must stay distinguishable from page-table mapping failures"
    );
}

/// Contract: ExecError Display messages remain stable for REPL/user diagnostics.
#[test_case]
fn test_exec_error_display_messages() {
    assert!(
        alloc::format!("{}", process::ExecError::InvalidName)
            == "invalid file name (expected 8.3 format)",
        "InvalidName display text must stay stable"
    );
    assert!(
        alloc::format!("{}", process::ExecError::OutOfMemory)
            == "out of memory while allocating program pages",
        "OutOfMemory display text must stay stable"
    );
    assert!(
        alloc::format!("{}", process::ExecError::Io) == "I/O error while loading program",
        "Io display text must stay stable"
    );
}

/// Contract: the loader returns the bundled user program and validates size bounds.
#[test_case]
fn test_load_program_image_reads_hello_bin() {
    kaos_kernel::drivers::ata::init();

    let image = process::load_program_image("hello.bin")
        .expect("hello.bin must be loadable via the VFS-backed process loader");
    assert!(!image.is_empty(), "loaded user image must contain bytes");
    assert!(
        image.len() <= process::USER_PROGRAM_MAX_IMAGE_SIZE,
        "loaded user image must fit configured executable mapping window"
    );
}

/// Contract: loader maps invalid-name failures to `ExecError::InvalidName`.
#[test_case]
fn test_load_program_image_maps_invalid_name_error() {
    let result = process::load_program_image("invalid.name.txt");
    assert!(
        matches!(result, Err(process::ExecError::InvalidName)),
        "invalid FAT short name must map to ExecError::InvalidName"
    );
}

/// Contract: loader maps missing-file failures to `ExecError::NotFound`.
#[test_case]
fn test_load_program_image_maps_not_found_error() {
    kaos_kernel::drivers::ata::init();

    let result = process::load_program_image("missing.bin");
    assert!(
        matches!(result, Err(process::ExecError::NotFound)),
        "missing filesystem entry must map to ExecError::NotFound"
    );
}

/// Contract: explicit image-length validator enforces non-empty lower bound and
/// user code window upper bound.
#[test_case]
fn test_validate_program_image_len_enforces_upper_bound() {
    assert!(
        matches!(
            process::validate_program_image_len(0),
            Err(process::ExecError::EmptyImage)
        ),
        "zero-length image must be rejected as EmptyImage"
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

/// Contract: image-length validator distinguishes empty image and oversized image.
#[test_case]
fn test_validate_program_image_len_distinguishes_empty_and_oversized() {
    let empty_err =
        process::validate_program_image_len(0).expect_err("zero-length image must be rejected");
    assert!(
        matches!(empty_err, process::ExecError::EmptyImage),
        "empty image must map to ExecError::EmptyImage"
    );

    let oversized_err =
        process::validate_program_image_len(process::USER_PROGRAM_MAX_IMAGE_SIZE + 1)
            .expect_err("oversized image must be rejected");
    assert!(
        matches!(oversized_err, process::ExecError::FileTooLarge),
        "oversized image must map to ExecError::FileTooLarge"
    );
}

/// Contract: public map API rejects empty and oversized images in all builds.
#[test_case]
fn test_map_program_image_into_user_address_space_enforces_image_bounds() {
    let empty = process::map_program_image_into_user_address_space(&[]);
    assert!(
        matches!(empty, Err(process::ExecError::EmptyImage)),
        "empty image must be rejected by public map API"
    );

    let oversized = vec![0u8; process::USER_PROGRAM_MAX_IMAGE_SIZE + 1];
    let oversized_result = process::map_program_image_into_user_address_space(&oversized);
    assert!(
        matches!(oversized_result, Err(process::ExecError::FileTooLarge)),
        "oversized image must be rejected by public map API"
    );
}

/// Minimal single-`PT_LOAD`-segment ELF64 image, used below to exercise the
/// loader's bootstrap-stack setup through the real (ELF-only) mapping path.
///
/// Deliberately not shared with `elf_test.rs`/`elf_loader_test.rs`'s builders:
/// each `kernel/tests/*.rs` file is a standalone test binary in this harness,
/// so there is no shared support module to pull a common builder from.
fn minimal_elf_image() -> Vec<u8> {
    const EHDR_SIZE: usize = 64;
    const PHDR_SIZE: usize = 56;
    const PAGE_SIZE_U64: u64 = 4096;
    const PF_X: u32 = 1;
    const PF_W: u32 = 2;
    const PF_R: u32 = 4;

    let vaddr = process::USER_PROGRAM_ENTRY_RIP; // page-aligned, inside the user code window
    let file_bytes: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
    let memsz = PAGE_SIZE_U64; // one full page; the tail past file_bytes is in-segment BSS

    let phoff = EHDR_SIZE as u64;
    let file_offset = phoff + PHDR_SIZE as u64;
    let mut image = vec![0u8; file_offset as usize + file_bytes.len()];

    image[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    image[4] = 2; // ELFCLASS64
    image[5] = 1; // ELFDATA2LSB
    image[6] = 1; // EV_CURRENT
    image[16..18].copy_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    image[18..20].copy_from_slice(&62u16.to_le_bytes()); // e_machine = EM_X86_64
    image[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    image[24..32].copy_from_slice(&vaddr.to_le_bytes()); // e_entry == segment base
    image[32..40].copy_from_slice(&phoff.to_le_bytes()); // e_phoff
    image[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes()); // e_phentsize
    image[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum

    let ph = phoff as usize;
    image[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    image[ph + 4..ph + 8].copy_from_slice(&(PF_R | PF_W | PF_X).to_le_bytes());
    image[ph + 8..ph + 16].copy_from_slice(&file_offset.to_le_bytes());
    image[ph + 16..ph + 24].copy_from_slice(&vaddr.to_le_bytes());
    image[ph + 24..ph + 32].copy_from_slice(&vaddr.to_le_bytes()); // p_paddr (ignored)
    image[ph + 32..ph + 40].copy_from_slice(&(file_bytes.len() as u64).to_le_bytes());
    image[ph + 40..ph + 48].copy_from_slice(&memsz.to_le_bytes());
    image[ph + 48..ph + 56].copy_from_slice(&PAGE_SIZE_U64.to_le_bytes());

    let start = file_offset as usize;
    image[start..start + file_bytes.len()].copy_from_slice(&file_bytes);

    image
}

/// Contract: a non-ELF byte blob is rejected outright -- there is no fallback
/// to a different loading strategy for input that fails ELF64 validation
/// (the legacy flat-binary path was removed once every in-tree program
/// migrated to ELF).
#[test_case]
fn test_map_program_image_into_user_address_space_rejects_non_elf_image() {
    let image: Vec<u8> = (0..200u32).map(|i| i as u8).collect();
    let result = process::map_program_image_into_user_address_space(&image);
    assert!(
        matches!(result, Err(process::ExecError::InvalidElfImage)),
        "non-ELF image must be rejected with InvalidElfImage, not silently mapped"
    );
}

/// Contract: loader maps a real ELF image's bootstrap user stack page as
/// user-accessible + writable, and zero-initializes it.
#[test_case]
fn test_map_program_image_into_user_address_space_zeroes_bootstrap_stack_page() {
    let image = minimal_elf_image();
    let loaded = process::map_program_image_into_user_address_space(&image)
        .expect("valid ELF image must map into fresh user CR3");

    let mut code_pfns = Vec::with_capacity(loaded.code_page_count);
    let mut stack_pfn = 0u64;
    vmm::with_address_space(loaded.cr3, || {
        // Collect mapped code PFNs for explicit post-teardown release.
        for idx in 0..loaded.code_page_count {
            let code_page_va = process::USER_PROGRAM_ENTRY_RIP + idx as u64 * pmm::PAGE_SIZE;
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
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - Loader mapped one writable bootstrap stack page at `stack_page_va`.
        // - Reading one full page is valid before any user code has executed.
        unsafe {
            let stack_base = stack_page_va as *const u8;
            for idx in 0..pmm::PAGE_SIZE as usize {
                let actual = core::ptr::read_volatile(stack_base.add(idx));
                assert!(
                    actual == 0,
                    "bootstrap stack byte at offset {} must be zero, got 0x{:02x}",
                    idx,
                    actual
                );
            }
        }
    });

    vmm::destroy_user_address_space(loaded.cr3, &[]);

    // Release code PFNs explicitly because current VMM teardown keeps USER_CODE
    // leaf PFNs reserved to support temporary kernel-text alias mappings.
    pmm::with_pmm(|mgr| {
        for pfn in code_pfns {
            let _ = mgr.release_pfn(pfn);
        }
        let _ = mgr.release_pfn(stack_pfn);
    });
}

/// Contract: `exec_from_vfs` maps image and spawns a scheduler user task.
#[test_case]
fn test_exec_from_vfs_spawns_user_task() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id =
        process::exec_from_vfs("hello.bin").expect("hello.bin exec path must spawn user task");

    assert!(
        scheduler::is_user_task(task_id),
        "exec_from_vfs must create a user-mode scheduler entry"
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

/// Contract: `exec_from_vfs` can spawn the bundled readline user program.
#[test_case]
fn test_exec_from_vfs_spawns_readline_user_task() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id = process::exec_from_vfs("readline.bin")
        .expect("readline.bin exec path must spawn user task");

    assert!(
        scheduler::is_user_task(task_id),
        "readline.bin exec must create a user-mode scheduler entry"
    );

    assert!(
        scheduler::terminate_task(task_id),
        "spawned readline user task must be terminatable for test cleanup"
    );
}

/// Contract: terminating an exec-loaded task releases loader-owned code PFNs.
#[test_case]
fn test_exec_from_vfs_terminate_releases_loader_code_pfn() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id =
        process::exec_from_vfs("hello.bin").expect("hello.bin exec path must spawn user task");
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

/// Contract: `exec_from_vfs` maps scheduler spawn errors to `ExecError::SpawnFailed`.
#[test_case]
fn test_exec_from_vfs_maps_spawn_failure_to_spawn_failed() {
    // Reset initialization state to guarantee a scheduler spawn error (NotInitialized).
    scheduler::reset_initialization_for_test();

    let result = process::exec_from_vfs("hello.bin");
    assert!(
        matches!(result, Err(process::ExecError::SpawnFailed)),
        "scheduler spawn failure must map to ExecError::SpawnFailed"
    );

    // Re-initialize scheduler after test to restore clean state for subsequent tests.
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());
}

/// Contract: `exec_from_vfs` maps invalid 8.3 short names to `ExecError::InvalidName`.
#[test_case]
fn test_exec_from_vfs_maps_invalid_name_error() {
    let result = process::exec_from_vfs("invalid.name.txt");
    assert!(
        matches!(result, Err(process::ExecError::InvalidName)),
        "invalid 8.3 input must fail early with ExecError::InvalidName"
    );
}

// ── M6: per-syscall authorization (Shutdown capability gate, issue #18) ──

/// Contract: `exec_from_vfs` (the `Exec` syscall path) always spawns an
/// unprivileged task.
///
/// Only the boot shell (spawned via `exec_from_image` with `privileged: true`
/// in `main.rs`) is granted the privileged-syscall capability. Every task
/// launched later through `Exec` — including recursively by the shell itself —
/// must default to unprivileged so it cannot call `Shutdown`.
#[test_case]
fn test_exec_from_vfs_spawns_unprivileged_task() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id =
        process::exec_from_vfs("hello.bin").expect("hello.bin exec path must spawn user task");

    assert!(
        !scheduler::is_task_privileged(task_id),
        "exec_from_vfs must never grant the privileged-syscall capability"
    );

    assert!(
        scheduler::terminate_task(task_id),
        "spawned user task must be terminatable for test cleanup"
    );
}

/// Contract: `Shutdown` is denied for an unprivileged running task, and the
/// machine does not shut down.
///
/// This is the concrete fix for issue #18 (M6 — no per-syscall
/// authorization): previously any ring-3 task could call `Shutdown` and power
/// off the machine. A task spawned through the `Exec` syscall path is never
/// privileged, so dispatching `Shutdown` while it is the running task must be
/// rejected with `SYSCALL_ERR_PERMISSION_DENIED` before
/// `arch::power::shutdown()` is ever reached.
#[test_case]
fn test_shutdown_denied_for_unprivileged_running_task() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_id =
        process::exec_from_vfs("hello.bin").expect("hello.bin exec path must spawn user task");
    assert!(
        !scheduler::is_task_privileged(task_id),
        "precondition: exec-spawned task must be unprivileged"
    );

    // Step 1: Select the freshly spawned task so it becomes the scheduler's
    // current task — the same state `syscall_shutdown_impl` inspects via
    // `scheduler::current_task_id()`.
    scheduler::start();
    let mut bootstrap = SavedRegisters::default();
    let selected = scheduler::on_timer_tick(&mut bootstrap as *mut SavedRegisters);
    assert_eq!(
        selected,
        scheduler::task_frame_ptr(task_id).expect("spawned task must expose a frame pointer"),
        "first tick must select the freshly spawned unprivileged task"
    );

    // Step 2: Dispatching Shutdown while this unprivileged task is current
    // must be denied, never reaching `arch::power::shutdown()`.
    let ret = syscall::dispatch(SyscallId::Shutdown as u64, 0, 0, 0, 0);
    assert_eq!(
        ret,
        syscall::SYSCALL_ERR_PERMISSION_DENIED,
        "unprivileged task must not be authorized to shut down the machine"
    );

    // Step 3: Prove the machine did not shut down: the task's lifecycle state
    // is untouched and the scheduler is still tracking it normally.
    assert_eq!(
        scheduler::task_state(task_id),
        Some(TaskState::Running),
        "a denied Shutdown call must not alter the calling task's lifecycle state"
    );

    assert!(
        scheduler::terminate_task(task_id),
        "spawned user task must be terminatable for test cleanup"
    );
}

/// Contract: a task spawned with the privileged capability satisfies the
/// `Shutdown` syscall's authorization check.
///
/// This test intentionally stops short of invoking the `Shutdown` syscall
/// itself: for an authorized caller, `syscall_shutdown_impl` calls
/// `arch::power::shutdown()`, which never returns and would tear down the
/// test QEMU instance before it could report a pass/fail result via
/// `isa-debug-exit`. Instead this verifies the exact state
/// `syscall_shutdown_impl` consults — `current_task_id()` combined with
/// `is_task_privileged()` — proving a privileged task reaches (and passes)
/// the same authorization gate that the previous test proves denies an
/// unprivileged one.
#[test_case]
fn test_privileged_running_task_passes_shutdown_authorization_check() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let task_id = scheduler::spawn_user_task(
        vmm::USER_CODE_BASE,
        vmm::USER_STACK_TOP - 16,
        user_cr3,
        true,
    )
    .expect("privileged user task should spawn");
    assert!(
        scheduler::is_task_privileged(task_id),
        "precondition: spawn_user_task(.., privileged=true) must grant the capability"
    );

    scheduler::start();
    let mut bootstrap = SavedRegisters::default();
    let selected = scheduler::on_timer_tick(&mut bootstrap as *mut SavedRegisters);
    assert_eq!(
        selected,
        scheduler::task_frame_ptr(task_id).expect("spawned task must expose a frame pointer"),
        "first tick must select the freshly spawned privileged task"
    );

    let current = scheduler::current_task_id().expect("a task must be current after selection");
    assert!(
        scheduler::is_task_privileged(current),
        "the currently running task must report privileged, matching the state \
         syscall_shutdown_impl's authorization check consults"
    );

    assert!(
        scheduler::terminate_task(task_id),
        "spawned user task must be terminatable for test cleanup"
    );
}

// ── M10: Exec rate limit / Wait scoping authorization (issue #53) ──

/// Contract: a task's `Exec` calls are rejected once it has already spawned
/// `MAX_CHILD_EXECS` (32) children.
///
/// This is the concrete fix for issue #53 (M10 — no per-syscall
/// authorization for `Exec`): previously any ring-3 task could loop `Exec`
/// without bound, spawning unprivileged children until scheduler slots or
/// PMM frames exhausted. `syscall_exec_impl` now increments a per-task
/// counter *before* touching user memory or the filesystem, so once the cap
/// is reached every further attempt is denied with
/// `SYSCALL_ERR_PERMISSION_DENIED` instead of `SYSCALL_ERR_INVALID_ARG` --
/// this test drives the syscall with a null name pointer (which would
/// otherwise always fail with `SYSCALL_ERR_INVALID_ARG`) so the distinct
/// error code observed at the cap boundary can only come from the rate
/// limiter, not from `read_user_string`.
#[test_case]
fn test_exec_denied_after_child_exec_cap_reached() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    // Step 1: Spawn a task and select it as the scheduler's current task, so
    // `syscall_exec_impl`'s `scheduler::current_task_id()` resolves to it
    // for every dispatch below.
    let task_id =
        process::exec_from_vfs("hello.bin").expect("caller task must spawn for this contract");

    scheduler::start();
    let mut bootstrap = SavedRegisters::default();
    let selected = scheduler::on_timer_tick(&mut bootstrap as *mut SavedRegisters);
    assert_eq!(
        selected,
        scheduler::task_frame_ptr(task_id).expect("spawned task must expose a frame pointer"),
        "first tick must select the freshly spawned task"
    );

    // Step 2: Exhaust the per-task Exec quota. Every call uses a null name
    // pointer, so calls that pass the rate-limit check still fail --
    // deterministically with SYSCALL_ERR_INVALID_ARG -- inside
    // `read_user_string`, without spawning any further tasks to clean up.
    for attempt in 0..32 {
        let ret = syscall::dispatch(SyscallId::Exec as u64, 0, 0, 0, 0);
        assert_eq!(
            ret,
            syscall::SYSCALL_ERR_INVALID_ARG,
            "attempt {} must still pass the rate limiter and fail on the null pointer",
            attempt
        );
    }

    // Step 3: The next attempt must be denied by the rate limiter itself --
    // observable because the error code changes from InvalidArg to
    // PermissionDenied despite passing the exact same (null) arguments.
    let ret = syscall::dispatch(SyscallId::Exec as u64, 0, 0, 0, 0);
    assert_eq!(
        ret,
        syscall::SYSCALL_ERR_PERMISSION_DENIED,
        "Exec must be denied once the per-task child cap is reached"
    );

    assert!(
        scheduler::terminate_task(task_id),
        "spawned caller task must be terminatable for test cleanup"
    );
}

/// Contract: an unprivileged task may `Wait` on a task it recorded as its own
/// `Exec`-spawned child.
///
/// `syscall_exec_impl` records the caller as `parent` on every successful
/// spawn; `syscall_wait_impl` must authorize a caller against exactly that
/// recording. The child is terminated before `Wait` is dispatched so the
/// call returns immediately via `wait_for_task_exit`'s already-absent fast
/// path instead of blocking the single-threaded test.
#[test_case]
fn test_wait_allowed_for_recorded_parent() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    // Step 1: Spawn and select the parent task as current.
    let parent_id =
        process::exec_from_vfs("hello.bin").expect("parent task must spawn for this contract");

    scheduler::start();
    let mut bootstrap = SavedRegisters::default();
    let selected = scheduler::on_timer_tick(&mut bootstrap as *mut SavedRegisters);
    assert_eq!(
        selected,
        scheduler::task_frame_ptr(parent_id).expect("spawned task must expose a frame pointer"),
        "first tick must select the freshly spawned parent task"
    );

    // Step 2: Spawn a second task and record the parent/child link exactly
    // as `syscall_exec_impl` would after a successful spawn. Spawning after
    // the tick above keeps `parent_id` the scheduler's current task.
    let child_id =
        process::exec_from_vfs("hello.bin").expect("child task must spawn for this contract");
    assert!(
        scheduler::set_task_parent(child_id, parent_id),
        "parent link must be recorded on a live child slot"
    );
    assert!(
        scheduler::is_parent_of(parent_id, child_id),
        "precondition: parent link must be observable via is_parent_of"
    );

    // Step 3: Terminate the child so `Wait` resolves immediately without
    // blocking this single-threaded test.
    assert!(
        scheduler::terminate_task(child_id),
        "child task must be terminatable ahead of the Wait call"
    );

    // Step 4: The recorded parent's Wait call must pass authorization and
    // reach `wait_for_task_exit`, returning success for the now-absent
    // child.
    let ret = syscall::dispatch(SyscallId::Wait as u64, child_id as u64, 0, 0, 0);
    assert_eq!(
        ret,
        syscall::SYSCALL_OK,
        "recorded parent must be authorized to Wait on its own child"
    );

    assert!(
        scheduler::terminate_task(parent_id),
        "parent task must be terminatable for test cleanup"
    );
}

/// Contract: `Wait` is denied for an unprivileged task that is not the
/// recorded parent of the target -- closing the M10 existence side-channel.
///
/// Before this fix, any ring-3 task could loop `Wait` over arbitrary task
/// ids to fingerprint which ids are currently alive (a live target blocks,
/// an absent one returns immediately). The target here is deliberately left
/// alive (never terminated) to prove the denial happens purely from the
/// authorization check, before `wait_for_task_exit` would ever distinguish
/// "alive" from "absent" -- an unauthorized caller gets the exact same
/// `SYSCALL_ERR_PERMISSION_DENIED` regardless of the target's true liveness.
#[test_case]
fn test_wait_denied_for_non_parent_unprivileged_task() {
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    // Step 1: Spawn and select the attacker task as current.
    let attacker_id =
        process::exec_from_vfs("hello.bin").expect("attacker task must spawn for this contract");

    scheduler::start();
    let mut bootstrap = SavedRegisters::default();
    let selected = scheduler::on_timer_tick(&mut bootstrap as *mut SavedRegisters);
    assert_eq!(
        selected,
        scheduler::task_frame_ptr(attacker_id).expect("spawned task must expose a frame pointer"),
        "first tick must select the freshly spawned attacker task"
    );

    // Step 2: Spawn an unrelated target task after the tick above, so the
    // attacker remains the scheduler's current task. No parent link is ever
    // recorded between the two.
    let target_id =
        process::exec_from_vfs("hello.bin").expect("target task must spawn for this contract");

    assert!(
        !scheduler::is_task_privileged(attacker_id),
        "precondition: exec-spawned attacker task must be unprivileged"
    );
    assert!(
        !scheduler::is_parent_of(attacker_id, target_id),
        "precondition: attacker must not be recorded as the target's parent"
    );

    // Step 3: The unauthorized Wait must be denied without ever consulting
    // the target's liveness -- the target is still alive at this point.
    let ret = syscall::dispatch(SyscallId::Wait as u64, target_id as u64, 0, 0, 0);
    assert_eq!(
        ret,
        syscall::SYSCALL_ERR_PERMISSION_DENIED,
        "non-parent unprivileged task must not be authorized to Wait on an unrelated task id"
    );
    assert_eq!(
        scheduler::task_state(target_id),
        Some(TaskState::Ready),
        "a denied Wait call must not alter the target task's lifecycle state"
    );

    // Cleanup.
    assert!(
        scheduler::terminate_task(target_id),
        "target task must be terminatable for test cleanup"
    );
    assert!(
        scheduler::terminate_task(attacker_id),
        "attacker task must be terminatable for test cleanup"
    );
}

/// Contract: user-program linker scripts place `.text._start` at image base.
#[test_case]
fn test_user_program_linker_scripts_prioritize_start_section() {
    let hello_linker = include_str!("../../user_programs/hello/link.ld");
    let readline_linker = include_str!("../../user_programs/readline/link.ld");

    fn assert_start_before_text(script: &str, name: &str) {
        let start_pos = script
            .find("*(.ltext._start)")
            .expect("linker script must define explicit .ltext._start placement");
        let text_pos = script
            .find("*(.ltext .ltext.*)")
            .expect("linker script must define generic .ltext placement");

        assert!(
            start_pos < text_pos,
            "{} linker script must place .ltext._start before .ltext.* to keep entry at image base",
            name
        );
    }

    assert_start_before_text(hello_linker, "hello");
    assert_start_before_text(readline_linker, "readline");
}
