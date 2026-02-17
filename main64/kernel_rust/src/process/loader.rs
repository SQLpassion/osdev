//! FAT12-backed loader for flat user-mode binaries.

use alloc::vec::Vec;

use crate::io::fat12::{self, Fat12Error};
use crate::memory::{pmm, vmm};
use crate::scheduler;

use super::types::{
    image_fits_user_code, ExecError, ExecResult, LoadedProgram, USER_PROGRAM_ENTRY_RIP,
    USER_PROGRAM_INITIAL_RSP,
};

/// Page size used for program-image pagination and copy window sizing.
const PAGE_SIZE_BYTES: usize = pmm::PAGE_SIZE as usize;
/// Bootstrap stack page mapped at the top of the user stack region.
const USER_STACK_BOOTSTRAP_PAGE_VA: u64 = vmm::USER_STACK_TOP - pmm::PAGE_SIZE;

/// Loads a flat user program from FAT12 and validates its image length.
///
/// Phase 4 scope:
/// - read file content from FAT12
/// - map FAT12-level errors into process exec errors
/// - reject images larger than the configured user code window
///
/// Caller requirements:
/// - ATA driver must be initialized before calling this function
/// - FAT12 layer must be initialized as part of normal kernel boot
///
/// Not part of this function:
/// - creating a dedicated user address space
/// - mapping code/stack pages
/// - spawning a scheduler task
pub fn load_program_image(file_name_8_3: &str) -> ExecResult<Vec<u8>> {
    let image = fat12::read_file(file_name_8_3).map_err(map_fat12_error)?;
    validate_program_image_len(image.len())?;
    Ok(image)
}

/// Loads a flat user program from FAT12 and maps/copies it into a fresh user CR3.
///
/// This function performs load + map/copy only and intentionally does not spawn
/// a scheduler task yet.
pub fn load_program_into_user_address_space(file_name_8_3: &str) -> ExecResult<LoadedProgram> {
    let image = load_program_image(file_name_8_3)?;
    map_program_image_into_user_address_space(&image)
}

/// End-to-end process exec path for FAT12-backed flat user binaries.
///
/// Flow:
/// 1. read + validate image from FAT12
/// 2. map/copy image into a fresh user address space
/// 3. spawn scheduler user task from the prepared descriptor
///
/// On spawn failure, any newly created user address space is destroyed to avoid
/// leaking process-owned mappings and frames.
pub fn exec_from_fat12(file_name_8_3: &str) -> ExecResult<usize> {
    let loaded = load_program_into_user_address_space(file_name_8_3)?;
    spawn_loaded_program(loaded)
}

/// Maps a validated flat image into a fresh user address space and copies bytes.
///
/// Mapping policy:
/// - code pages start at [`USER_PROGRAM_ENTRY_RIP`] and are finalized read-only
/// - one bootstrap stack page is mapped at the top of user stack as writable
/// - returned descriptor contains CR3/RIP/RSP + image length for follow-up spawn
pub fn map_program_image_into_user_address_space(image: &[u8]) -> ExecResult<LoadedProgram> {
    // Validate early so no PMM/VMM state is touched for oversized images.
    validate_program_image_len(image.len())?;

    // Each process gets its own CR3 root cloned from the current kernel baseline.
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    if user_cr3 == 0 {
        return Err(ExecError::AddressSpaceCreateFailed);
    }

    // Track every allocated PFN so we can reliably roll back on any later error.
    let code_page_count = page_count_for_len(image.len());
    let mut code_pfns = Vec::with_capacity(code_page_count);
    let mut stack_pfn = None::<u64>;

    // Transaction-style setup:
    // - success returns a fully initialized descriptor
    // - failure triggers explicit cleanup below
    let result = (|| -> ExecResult<LoadedProgram> {
        // Allocate all required code backing frames first so mapping phase cannot
        // fail mid-way due to late frame exhaustion.
        for _ in 0..code_page_count {
            code_pfns.push(alloc_frame_pfn()?);
        }

        // Allocate one initial user stack page so first ring-3 pushes are valid.
        stack_pfn = Some(alloc_frame_pfn()?);
        let stack_pfn = stack_pfn.ok_or(ExecError::MappingFailed)?;

        // Perform mapping and copy while target CR3 is active.
        vmm::with_address_space(user_cr3, || -> ExecResult<()> {
            // Phase 1: map code pages writable to allow zero-fill + image copy.
            for (idx, pfn) in code_pfns.iter().copied().enumerate() {
                let page_va = USER_PROGRAM_ENTRY_RIP + idx as u64 * pmm::PAGE_SIZE;
                vmm::map_user_page(page_va, pfn, true).map_err(|_| ExecError::MappingFailed)?;
            }

            // Keep one writable bootstrap stack page at top-of-stack region.
            vmm::map_user_page(USER_STACK_BOOTSTRAP_PAGE_VA, stack_pfn, true)
                .map_err(|_| ExecError::MappingFailed)?;

            if code_page_count > 0 {
                let mapped_code_bytes = code_page_count * PAGE_SIZE_BYTES;

                // SAFETY:
                // - Code-page mappings above ensure `[USER_PROGRAM_ENTRY_RIP, +mapped_code_bytes)`
                //   is writable in the currently active address space.
                // - Zeroing removes stale frame content before exposing bytes to user mode.
                unsafe {
                    core::ptr::write_bytes(USER_PROGRAM_ENTRY_RIP as *mut u8, 0, mapped_code_bytes);
                }

                // SAFETY:
                // - Source slice `image` is valid for `image.len()` bytes.
                // - Destination starts at `USER_PROGRAM_ENTRY_RIP` and remains writable
                //   for at least `image.len()` bytes in current CR3.
                // - Source and destination do not overlap.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        image.as_ptr(),
                        USER_PROGRAM_ENTRY_RIP as *mut u8,
                        image.len(),
                    );
                }

                // Phase 2: tighten permissions after copy (code should be read-only).
                for (idx, pfn) in code_pfns.iter().copied().enumerate() {
                    let page_va = USER_PROGRAM_ENTRY_RIP + idx as u64 * pmm::PAGE_SIZE;
                    vmm::map_user_page(page_va, pfn, false)
                        .map_err(|_| ExecError::MappingFailed)?;
                }
            }

            Ok(())
        })?;

        // Return the materialized process image descriptor for later spawn logic.
        Ok(LoadedProgram::new(
            user_cr3,
            USER_PROGRAM_ENTRY_RIP,
            USER_PROGRAM_INITIAL_RSP,
            image.len(),
        ))
    })();

    if result.is_err() {
        // Roll back both page-table changes and reserved physical frames.
        cleanup_failed_program_mapping(user_cr3, &code_pfns, stack_pfn);
    }

    result
}

/// Validates that a program image length fits inside the user executable window.
#[inline]
pub const fn validate_program_image_len(image_len: usize) -> ExecResult<()> {
    if image_fits_user_code(image_len) {
        Ok(())
    } else {
        Err(ExecError::FileTooLarge)
    }
}

/// Maps FAT12-specific load errors into process-level exec errors.
///
/// This translation layer keeps callers independent of filesystem internals.
fn map_fat12_error(error: Fat12Error) -> ExecError {
    match error {
        Fat12Error::InvalidFileName => ExecError::InvalidName,
        Fat12Error::NotFound => ExecError::NotFound,
        Fat12Error::IsDirectory => ExecError::IsDirectory,
        Fat12Error::Ata(_)
        | Fat12Error::CorruptDirectoryEntry
        | Fat12Error::CorruptFatChain
        | Fat12Error::UnexpectedEof => ExecError::Io,
    }
}

/// Returns the number of pages required to hold `image_len` bytes.
///
/// Uses ceil-division and returns `0` for an empty image.
#[inline]
const fn page_count_for_len(image_len: usize) -> usize {
    image_len.div_ceil(PAGE_SIZE_BYTES)
}

/// Allocates one physical frame and returns its PFN.
///
/// Allocation failure is reported as `ExecError::MappingFailed`.
fn alloc_frame_pfn() -> ExecResult<u64> {
    pmm::with_pmm(|mgr| mgr.alloc_frame().map(|frame| frame.pfn)).ok_or(ExecError::MappingFailed)
}

/// Best-effort rollback for partially created user mappings.
///
/// This releases page-table state via VMM teardown and then returns explicitly
/// tracked frames to PMM when still reserved.
fn cleanup_failed_program_mapping(user_cr3: u64, code_pfns: &[u64], stack_pfn: Option<u64>) {
    vmm::destroy_user_address_space(user_cr3);

    pmm::with_pmm(|mgr| {
        for pfn in code_pfns.iter().copied() {
            let _ = mgr.release_pfn(pfn);
        }

        if let Some(pfn) = stack_pfn {
            let _ = mgr.release_pfn(pfn);
        }
    });
}

/// Spawns a scheduler user task from an already prepared loaded-program descriptor.
///
/// Ownership contract:
/// - on success, scheduler owns `loaded.cr3` lifecycle via task teardown
/// - on failure, this function destroys `loaded.cr3` immediately
fn spawn_loaded_program(loaded: LoadedProgram) -> ExecResult<usize> {
    match scheduler::spawn_user_task_owning_code(loaded.entry_rip, loaded.user_rsp, loaded.cr3) {
        Ok(task_id) => Ok(task_id),
        Err(_) => {
            vmm::destroy_user_address_space_with_options(loaded.cr3, true);
            Err(ExecError::SpawnFailed)
        }
    }
}
