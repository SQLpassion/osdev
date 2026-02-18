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

/// Virtual address of the single stack page mapped when a user process starts.
///
/// Layout of the user stack region (high → low):
/// ```text
/// USER_STACK_TOP              = 0x0000_7FFF_F000_0000  (exclusive upper bound)
/// USER_STACK_BOOTSTRAP_PAGE_VA= 0x0000_7FFF_EFFF_F000  ← this page (4 KiB)
///     ...                                               (unmapped; grows on demand)
/// USER_STACK_BASE             = 0x0000_7FFF_EFF0_0000  (1 MiB stack region start)
/// USER_STACK_GUARD_BASE       = 0x0000_7FFF_EFEF_F000  (4 KiB guard page)
/// ```
///
/// The initial RSP is set to `USER_STACK_TOP - 16` (16-byte ABI alignment).
/// The first user push/call therefore lands inside this page, so mapping exactly
/// one page here is sufficient to let the program start without an immediate
/// page fault.  Additional stack pages are faulted in on demand as RSP grows
/// downward — not yet implemented; this single page is all that exists today.
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
///
/// # Preconditions
/// - `image` must be non-empty and satisfy `image_fits_user_code(image.len())`.
///   The normal entry point `load_program_image()` enforces this via
///   `validate_program_image_len()` before calling this function.
pub fn map_program_image_into_user_address_space(image: &[u8]) -> ExecResult<LoadedProgram> {
    // Debug-only guard: the validated public entry point load_program_image()
    // already rejects empty and oversized images before reaching this function.
    // The assert catches direct callers that bypass that validation.
    debug_assert!(
        !image.is_empty() && image_fits_user_code(image.len()),
        "map_program_image_into_user_address_space: precondition violated (image_len={})",
        image.len(),
    );

    // Each process gets its own CR3 root cloned from the current kernel baseline.
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    if user_cr3 == 0 {
        return Err(ExecError::AddressSpaceCreateFailed);
    }

    // Track every allocated PFN so we can reliably roll back on any later error.
    let code_page_count = page_count_for_len(image.len());
    let mut code_pfns = Vec::with_capacity(code_page_count);
    let mut stack_pfn = None::<u64>;
    let mut mapped_code_pages = 0usize;
    let mut stack_mapped = false;

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
                vmm::map_user_page(page_va, pfn, true).map_err(|e| {
                    crate::logging::logln(
                        "loader",
                        format_args!(
                            "LOADER: map_user_page(code page {}, va={:#x}, pfn={:#x}, writable=true) failed: {:?}",
                            idx, page_va, pfn, e
                        ),
                    );
                    ExecError::MappingFailed
                })?;

                // Track which code pages are already mapped into this CR3.
                // Rollback uses this count to release only never-mapped PFNs explicitly.
                mapped_code_pages += 1;
            }

            // Keep one writable bootstrap stack page at top-of-stack region.
            vmm::map_user_page(USER_STACK_BOOTSTRAP_PAGE_VA, stack_pfn, true)
                .map_err(|e| {
                    crate::logging::logln(
                        "loader",
                        format_args!(
                            "LOADER: map_user_page(stack, va={:#x}, pfn={:#x}, writable=true) failed: {:?}",
                            USER_STACK_BOOTSTRAP_PAGE_VA, stack_pfn, e
                        ),
                    );
                    ExecError::MappingFailed
                })?;

            // Mark stack bootstrap page as mapped so rollback knows whether PMM
            // release is handled by VMM teardown or needs explicit release.
            stack_mapped = true;

            // The length validator above rejects empty images, so there must be
            // at least one code page to materialize here.
            debug_assert!(
                code_page_count > 0,
                "validated image must allocate at least one code page"
            );

            let mapped_code_bytes = code_page_count * PAGE_SIZE_BYTES;

            // SAFETY:
            // - Source slice `image` is valid for `image.len()` bytes.
            // - Destination starts at `USER_PROGRAM_ENTRY_RIP` and remains writable
            //   for at least `image.len()` bytes in current CR3.
            // - Source and destination do not overlap (kernel image vs. user VA range).
            unsafe {
                core::ptr::copy_nonoverlapping(
                    image.as_ptr(),
                    USER_PROGRAM_ENTRY_RIP as *mut u8,
                    image.len(),
                );
            }

            // Zero only the tail beyond the image to clear stale frame content.
            // The image bytes copied above already overwrite the leading portion,
            // so zeroing those bytes again would be redundant work.
            //
            // SAFETY:
            // - Code-page mappings above ensure `[USER_PROGRAM_ENTRY_RIP, +mapped_code_bytes)`
            //   is writable in the currently active address space.
            // - `image.len() <= mapped_code_bytes` is guaranteed by `page_count_for_len`.
            let tail_len = mapped_code_bytes - image.len();
            if tail_len > 0 {
                unsafe {
                    core::ptr::write_bytes(
                        (USER_PROGRAM_ENTRY_RIP as usize + image.len()) as *mut u8,
                        0,
                        tail_len,
                    );
                }
            }

            // Phase 2: tighten permissions after copy (code should be read-only).
            for (idx, pfn) in code_pfns.iter().copied().enumerate() {
                let page_va = USER_PROGRAM_ENTRY_RIP + idx as u64 * pmm::PAGE_SIZE;
                vmm::map_user_page(page_va, pfn, false).map_err(|e| {
                    crate::logging::logln(
                        "loader",
                        format_args!(
                            "LOADER: map_user_page(code page {}, va={:#x}, pfn={:#x}, writable=false) failed: {:?}",
                            idx, page_va, pfn, e
                        ),
                    );
                    ExecError::MappingFailed
                })?;
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
        // Roll back page-table state and release still-unmapped physical frames.
        cleanup_failed_program_mapping(
            user_cr3,
            &code_pfns,
            stack_pfn,
            mapped_code_pages,
            stack_mapped,
        );
    }

    result
}

/// Validates that a program image length is non-empty and fits inside the user
/// executable window.
///
/// A zero-length image is rejected because there is no code to execute.
/// An image exceeding [`USER_PROGRAM_MAX_IMAGE_SIZE`] is rejected because it
/// would overflow the fixed user code region.
#[inline]
pub const fn validate_program_image_len(image_len: usize) -> ExecResult<()> {
    // Reject a structurally empty image with a dedicated error so callers can
    // surface a precise user-facing message.
    if image_len == 0 {
        Err(ExecError::EmptyImage)
    } else if !image_fits_user_code(image_len) {
        // Reject oversized images that would overflow the fixed USER_CODE area.
        Err(ExecError::FileTooLarge)
    } else {
        Ok(())
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
///
/// The PMM only manages regions at or above 1 MiB (`KERNEL_OFFSET`), so PFN 0
/// (physical address 0x0000, IVT/BIOS data area) is structurally unreachable.
/// The assertion below makes any violation of that invariant visible immediately.
fn alloc_frame_pfn() -> ExecResult<u64> {
    let pfn = pmm::with_pmm(|mgr| mgr.alloc_frame().map(|frame| frame.pfn))
        .ok_or(ExecError::MappingFailed)?;

    // PFN 0 maps to physical address 0x0 (IVT/BIOS Data Area). Mapping user
    // pages there would corrupt low memory and be a security vulnerability.
    // This should never trigger given the PMM's region filter, but guard
    // against future PMM changes that might break the invariant.
    debug_assert!(pfn != 0, "PMM returned PFN 0 (reserved low memory)");
    if pfn == 0 {
        return Err(ExecError::MappingFailed);
    }

    Ok(pfn)
}

/// Best-effort rollback for partially created user mappings.
///
/// Uses the same explicit owned-code teardown policy as normal process exit and
/// then releases only frames that were allocated but never mapped.
fn cleanup_failed_program_mapping(
    user_cr3: u64,
    code_pfns: &[u64],
    stack_pfn: Option<u64>,
    mapped_code_pages: usize,
    stack_mapped: bool,
) {
    // Teardown mapped ranges with owned-code policy:
    // - mapped code PFNs are released (owned image contract),
    // - mapped stack PFNs are released (always process-owned),
    // - page-table hierarchy + CR3 root are released.
    vmm::destroy_user_address_space_with_options(user_cr3, true);

    pmm::with_pmm(|mgr| {
        // Any frames beyond `mapped_code_pages` were never inserted into page
        // tables and therefore are not covered by VMM teardown.
        for pfn in code_pfns.iter().copied().skip(mapped_code_pages) {
            let _ = mgr.release_pfn(pfn);
        }

        // Apply the same rule to the optional bootstrap stack frame.
        if !stack_mapped {
            if let Some(pfn) = stack_pfn {
                let _ = mgr.release_pfn(pfn);
            }
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
        Err(e) => {
            crate::logging::logln(
                "loader",
                format_args!(
                    "LOADER: spawn_user_task_owning_code(rip={:#x}, rsp={:#x}, cr3={:#x}) failed: {:?}",
                    loaded.entry_rip, loaded.user_rsp, loaded.cr3, e
                ),
            );
            vmm::destroy_user_address_space_with_options(loaded.cr3, true);
            Err(ExecError::SpawnFailed)
        }
    }
}
