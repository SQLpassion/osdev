//! VFS-backed loader for user-mode programs.
//!
//! Every on-disk program is a static `ET_EXEC`/`EM_X86_64` ELF64 executable,
//! parsed by [`super::elf::parse_elf64`] and mapped one `PT_LOAD` segment at
//! a time, each with its own `writable`/`no_execute` permissions derived from
//! `p_flags`.
//!
//! A prior revision of this loader also supported a legacy flat-binary format
//! (pre-migration `objcopy -O binary` output) as a gradual-rollout fallback
//! while individual programs migrated one at a time. That fallback was
//! removed once all in-tree programs shipped as ELF; any image that fails
//! ELF64 validation is now rejected with [`ExecError::InvalidElfImage`]
//! instead of falling back to a different loading strategy.

use alloc::vec::Vec;

use crate::io::vfs::{self, FsError};
use crate::memory::{pmm, vmm};
use crate::scheduler;

use super::elf::{self, ElfImage};
use super::types::{
    image_fits_user_code, ExecError, ExecResult, LoadedProgram, USER_PROGRAM_INITIAL_RSP,
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

/// Per-segment transaction state for the ELF mapping path.
///
/// One entry per validated `PT_LOAD` segment, tracking which physical frames
/// are allocated and how many have actually been inserted into page tables so
/// far, so a failure partway through mapping can be unwound precisely.
struct ElfSegmentState {
    /// Physical frames backing this segment, one per page, in VA order.
    pfns: Vec<u64>,
    /// How many of `pfns` (from the front) have been inserted into page tables.
    mapped_pages: usize,
    /// Final `writable` permission derived from `p_flags` (`PF_W`).
    writable: bool,
    /// Final `executable` permission derived from `p_flags` (`PF_X`).
    executable: bool,
}

/// Transaction state for the ELF mapping path: one [`ElfSegmentState`] per
/// `PT_LOAD` segment plus the shared bootstrap stack page.
struct ElfMapState {
    segments: Vec<ElfSegmentState>,
    stack_pfn: Option<u64>,
    stack_mapped: bool,
}

/// Loads a user program from the mounted filesystem and validates its image length.
///
/// Scope:
/// - read file content through the VFS facade (FAT32 on both boot paths)
/// - map filesystem errors into process exec errors
/// - reject images larger than the configured user code window
///
/// Caller requirements:
/// - the active block device and a filesystem must be mounted (normal kernel boot)
///
/// Not part of this function:
/// - creating a dedicated user address space
/// - mapping code/stack pages
/// - spawning a scheduler task
pub fn load_program_image(file_name_8_3: &str) -> ExecResult<Vec<u8>> {
    let image = vfs::read_file(file_name_8_3).map_err(map_fs_error)?;
    validate_program_image_len(image.len())?;
    Ok(image)
}

/// Loads a user program from the filesystem and maps/copies it into a fresh user CR3.
///
/// This function performs load + map/copy only and intentionally does not spawn
/// a scheduler task yet.
pub fn load_program_into_user_address_space(file_name_8_3: &str) -> ExecResult<LoadedProgram> {
    let image = load_program_image(file_name_8_3)?;
    map_program_image_into_user_address_space(&image)
}

/// End-to-end process exec path that reads a user binary by name from the VFS.
///
/// Flow:
/// 1. read + validate image through the VFS facade (FAT32 on both boot paths)
/// 2. map/copy image into a fresh user address space
/// 3. spawn scheduler user task from the prepared descriptor
///
/// On spawn failure, any newly created user address space is destroyed to avoid
/// leaking process-owned mappings and frames.
///
/// Tasks spawned through this path (i.e. via the `Exec` syscall) are always
/// unprivileged: only the boot shell spawned by [`exec_from_image`] is granted
/// the privileged-syscall capability (see M6, `docs/CODE_REVIEW_2026-07-23.md`).
pub fn exec_from_vfs(file_name_8_3: &str) -> ExecResult<usize> {
    let loaded = load_program_into_user_address_space(file_name_8_3)?;
    spawn_loaded_program(loaded, false)
}

/// End-to-end process exec path for an already-loaded user binary.
///
/// Unlike [`exec_from_vfs`], this takes the program bytes from the caller
/// instead of reading them from the filesystem.  It is used by the boot path to
/// launch the initial shell from an image already read into a buffer.
///
/// Flow:
/// 1. map/copy the provided image into a fresh user address space
///    (`map_program_image_into_user_address_space` validates the image length)
/// 2. spawn a scheduler user task from the prepared descriptor
///
/// On spawn failure, the newly created user address space is destroyed to avoid
/// leaking process-owned mappings and frames.
///
/// `privileged` is forwarded unchanged to the scheduler spawn call (see M6,
/// `docs/CODE_REVIEW_2026-07-23.md`). The boot path passes `true` here for the
/// initial shell task; callers that load additional images through this
/// function for any other purpose should pass `false`.
pub fn exec_from_image(image: &[u8], privileged: bool) -> ExecResult<usize> {
    let loaded = map_program_image_into_user_address_space(image)?;
    spawn_loaded_program(loaded, privileged)
}

/// Maps a validated program image into a fresh user address space and copies bytes.
///
/// The image is parsed as ELF64 and mapped one `PT_LOAD` segment at a time
/// (see [`map_elf_program_image`]). Any image that fails ELF64 validation —
/// including one that isn't ELF at all — is rejected with
/// [`ExecError::InvalidElfImage`]; there is no fallback to a different
/// loading strategy.
///
/// # Preconditions
/// - `image` must be non-empty and satisfy `image_fits_user_code(image.len())`.
///   The normal entry point `load_program_image()` enforces this via
///   `validate_program_image_len()` before calling this function.
pub fn map_program_image_into_user_address_space(image: &[u8]) -> ExecResult<LoadedProgram> {
    // Public API guard: callers may bypass `load_program_image()`, so enforce
    // the non-empty / size-bounded image contract in all build profiles.
    validate_program_image_len(image.len())?;

    map_elf_program_image(image)
}

/// Parses and maps a validated ELF64 image, one `PT_LOAD` segment at a time.
///
/// Mapping policy:
/// - every segment is mapped writable+executable first so copy/zero is valid
///   regardless of its final permissions, then tightened to `p_flags`
///   (`writable = PF_W`, `executable = PF_X`) once initialized
/// - one bootstrap stack page is mapped at the top of user stack as writable
/// - returned descriptor's `entry_rip` comes from `e_entry`, already validated
///   by [`elf::parse_elf64`] to fall inside an executable segment
fn map_elf_program_image(image: &[u8]) -> ExecResult<LoadedProgram> {
    let elf_image = elf::parse_elf64(image).map_err(|_| ExecError::InvalidElfImage)?;

    // Each process gets its own CR3 root cloned from the current kernel baseline.
    // The clone helper panics on OOM and never returns 0.
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    let mut state = ElfMapState {
        segments: elf_image
            .segments
            .iter()
            .map(|seg| ElfSegmentState {
                pfns: Vec::new(),
                mapped_pages: 0,
                writable: seg.writable(),
                executable: seg.executable(),
            })
            .collect(),
        stack_pfn: None,
        stack_mapped: false,
    };

    let result = try_map_elf_program_image(user_cr3, image, &elf_image, &mut state);

    if result.is_err() {
        cleanup_failed_elf_mapping(user_cr3, &state);
    }

    result
}

/// Performs the fallible per-segment map/copy transaction for a validated ELF image.
fn try_map_elf_program_image(
    user_cr3: u64,
    image: &[u8],
    elf_image: &ElfImage,
    state: &mut ElfMapState,
) -> ExecResult<LoadedProgram> {
    // Step 1: Allocate every segment's frames plus the stack frame in a single
    // PMM lock scope, to avoid repeated lock/unlock overhead in the hot
    // allocation loop.
    let segment_page_counts: Vec<usize> = elf_image.segments.iter().map(|s| s.page_count()).collect();
    let (segment_pfns, stack_pfn) = alloc_elf_frames(&segment_page_counts)?;

    // Wire allocated PFNs into `state` up front so a mapping failure below can
    // still see exactly which frames this transaction owns.
    for (seg_state, pfns) in state.segments.iter_mut().zip(segment_pfns) {
        seg_state.pfns = pfns;
    }
    state.stack_pfn = Some(stack_pfn);

    // Step 2: Activate target CR3 and perform map/copy/remap sequence.
    vmm::with_address_space(user_cr3, || -> ExecResult<()> {
        for (seg_idx, seg) in elf_image.segments.iter().enumerate() {
            let seg_state = &mut state.segments[seg_idx];

            // Step 2a: map every page in this segment writable+executable so
            // the copy/zero step below is valid regardless of the segment's
            // final permissions ("map writable first, tighten after"). Safe
            // because this whole closure runs with interrupts disabled (see
            // `with_address_space`), so no user code can observe the
            // transient over-permissive mapping.
            for (page_idx, &pfn) in seg_state.pfns.iter().enumerate() {
                let page_va = seg.vaddr + page_idx as u64 * pmm::PAGE_SIZE;
                vmm::map_user_code_page(page_va, pfn, true, true).map_err(|e| {
                    crate::logging::logln(
                        "loader",
                        format_args!(
                            "LOADER: map_user_code_page(segment {}, page {}, va={:#x}, pfn={:#x}) failed: {:?}",
                            seg_idx, page_idx, page_va, pfn, e
                        ),
                    );
                    ExecError::MappingFailed
                })?;

                // Track successful mappings for partial rollback.
                seg_state.mapped_pages += 1;
            }

            // Step 2b: copy `p_filesz` bytes, then zero from the end of the
            // copied bytes through the page-rounded segment end in a single
            // pass. This one write covers both the in-segment BSS tail
            // (`p_filesz..p_memsz`) and the page padding beyond `p_memsz`,
            // since both ranges are contiguous and already validated by
            // `parse_elf64` to lie inside this segment's mapped pages.
            let file_start = seg.offset as usize;
            let file_end = file_start + seg.filesz as usize;

            // SAFETY:
            // - `image[file_start..file_end]` was bounds-checked by `parse_elf64`
            //   (`SegmentFileRangeOutOfBounds` is rejected there).
            // - Destination `seg.vaddr` is mapped writable for `seg.filesz` bytes
            //   by the loop in step 2a.
            // - Source (kernel image buffer) and destination (user VA) do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    image[file_start..file_end].as_ptr(),
                    seg.vaddr as *mut u8,
                    seg.filesz as usize,
                );
            }

            let zero_start = seg.vaddr + seg.filesz;
            let zero_len = (seg.mapped_end() - zero_start) as usize;
            if zero_len > 0 {
                // SAFETY:
                // - `[zero_start, seg.mapped_end())` is mapped writable by step 2a.
                // - This is the segment's declared BSS (`p_filesz..p_memsz`) plus the
                //   page-tail padding beyond `p_memsz`, contiguous with no gap.
                unsafe {
                    core::ptr::write_bytes(zero_start as *mut u8, 0, zero_len);
                }
            }

            // Step 2c: tighten every page in this segment to its final ELF
            // permissions now that copy/zero is done.
            for (page_idx, &pfn) in seg_state.pfns.iter().enumerate() {
                let page_va = seg.vaddr + page_idx as u64 * pmm::PAGE_SIZE;
                vmm::map_user_code_page(page_va, pfn, seg_state.writable, seg_state.executable)
                    .map_err(|e| {
                        crate::logging::logln(
                            "loader",
                            format_args!(
                                "LOADER: map_user_code_page(finalize segment {}, page {}, va={:#x}) failed: {:?}",
                                seg_idx, page_idx, page_va, e
                            ),
                        );
                        ExecError::MappingFailed
                    })?;
            }
        }

        // Step 2d: map writable bootstrap stack page for initial ring-3 entry.
        vmm::map_user_page(USER_STACK_BOOTSTRAP_PAGE_VA, stack_pfn, true).map_err(|e| {
            crate::logging::logln(
                "loader",
                format_args!(
                    "LOADER: map_user_page(stack, va={:#x}, pfn={:#x}, writable=true) failed: {:?}",
                    USER_STACK_BOOTSTRAP_PAGE_VA, stack_pfn, e
                ),
            );
            ExecError::MappingFailed
        })?;

        // Mark stack as mapped so rollback delegates release to VMM teardown.
        state.stack_mapped = true;

        // Step 2e: zero the bootstrap stack page before first ring-3 use.
        // SAFETY:
        // - This requires `unsafe` because it writes bytes through a raw virtual-address pointer.
        // - `USER_STACK_BOOTSTRAP_PAGE_VA` is mapped writable in current CR3.
        // - Exactly one page (`PAGE_SIZE_BYTES`) is mapped for bootstrap stack.
        unsafe {
            core::ptr::write_bytes(USER_STACK_BOOTSTRAP_PAGE_VA as *mut u8, 0, PAGE_SIZE_BYTES);
        }

        Ok(())
    })?;

    // Step 3: Return finalized process descriptor for scheduler spawn step.
    // `code_page_count` is the sum of every segment's mapped pages -- see its
    // doc comment on `LoadedProgram` for why an approximate value here is safe.
    let total_pages: usize = state.segments.iter().map(|s| s.pfns.len()).sum();

    Ok(LoadedProgram::new(
        user_cr3,
        elf_image.entry,
        USER_PROGRAM_INITIAL_RSP,
        image.len(),
        total_pages,
    ))
}

/// Allocates physical frames for every ELF segment plus one bootstrap stack
/// frame, in a single PMM lock scope.
///
/// On partial allocation failure, all already-allocated PFNs in this
/// transaction are released before returning, so the caller receives
/// all-or-nothing behavior.
fn alloc_elf_frames(segment_page_counts: &[usize]) -> ExecResult<(Vec<Vec<u64>>, u64)> {
    // Allocate the outer/inner Vecs on the kernel heap before acquiring the PMM
    // lock, avoiding lock-ordering inversion deadlocks (PMM lock -> HEAP lock).
    let mut segment_pfns: Vec<Vec<u64>> = segment_page_counts
        .iter()
        .map(|&n| Vec::with_capacity(n))
        .collect();

    let stack_pfn = pmm::with_pmm(|mgr| {
        for seg_idx in 0..segment_pfns.len() {
            for _ in 0..segment_page_counts[seg_idx] {
                let pfn = match mgr.alloc_frame().map(|frame| frame.pfn) {
                    Some(pfn) => pfn,
                    None => {
                        release_elf_pfns(mgr, &segment_pfns);
                        return Err(ExecError::OutOfMemory);
                    }
                };

                // PFN 0 maps to physical address 0x0 (IVT/BIOS Data Area). Mapping
                // user pages there would corrupt low memory and be a security
                // vulnerability. Treat this as a hard PMM invariant violation.
                assert!(pfn != 0, "PMM returned PFN 0 (reserved low memory)");

                segment_pfns[seg_idx].push(pfn);
            }
        }

        let stack_pfn = match mgr.alloc_frame().map(|frame| frame.pfn) {
            Some(pfn) => pfn,
            None => {
                release_elf_pfns(mgr, &segment_pfns);
                return Err(ExecError::OutOfMemory);
            }
        };

        assert!(
            stack_pfn != 0,
            "PMM returned PFN 0 (reserved low memory) for stack frame"
        );

        Ok(stack_pfn)
    })?;

    Ok((segment_pfns, stack_pfn))
}

/// Releases every PFN across all segments back to the PMM.
///
/// Used only on allocation failure inside [`alloc_elf_frames`], while still
/// holding the PMM lock passed in as `mgr`.
fn release_elf_pfns(mgr: &mut pmm::PhysicalMemoryManager, segment_pfns: &[Vec<u64>]) {
    for pfns in segment_pfns {
        for &pfn in pfns {
            let _ = mgr.release_pfn(pfn);
        }
    }
}

/// Best-effort rollback for a partially created ELF user mapping.
///
/// Tears down the mapped prefix of each segment (plus the stack, if mapped)
/// through the normal VMM teardown path, then releases any frames that were
/// allocated but never inserted into page tables.
fn cleanup_failed_elf_mapping(user_cr3: u64, state: &ElfMapState) {
    // `destroy_user_address_space_with_page_counts`'s catch-all scan (see its
    // doc comment) reclaims any present mapping in the whole code window
    // regardless of this count, so summing `mapped_pages` here is a fast-path
    // hint, not a correctness requirement.
    let total_mapped_pages: usize = state.segments.iter().map(|s| s.mapped_pages).sum();
    let stack_pages_mapped = if state.stack_mapped { 1 } else { 0 };
    vmm::destroy_user_address_space_with_page_counts(
        user_cr3,
        total_mapped_pages,
        stack_pages_mapped,
    );

    pmm::with_pmm(|mgr| {
        for seg_state in &state.segments {
            // Any frame beyond `mapped_pages` was never inserted into page
            // tables and therefore is not covered by VMM teardown.
            for &pfn in seg_state.pfns.iter().skip(seg_state.mapped_pages) {
                let _ = mgr.release_pfn(pfn);
            }
        }

        // Apply the same rule to the optional bootstrap stack frame.
        if !state.stack_mapped {
            if let Some(pfn) = state.stack_pfn {
                let _ = mgr.release_pfn(pfn);
            }
        }
    });
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

/// Maps VFS load errors into process-level exec errors.
///
/// This translation layer keeps callers independent of filesystem internals.
fn map_fs_error(error: FsError) -> ExecError {
    match error {
        FsError::InvalidName => ExecError::InvalidName,
        FsError::NotFound => ExecError::NotFound,
        FsError::Unsupported
        | FsError::NotMounted
        | FsError::InvalidFd
        | FsError::Io
        | FsError::NotFat32
        | FsError::IsDirectory
        | FsError::BadChain
        | FsError::TooLarge => ExecError::Io,
    }
}

/// Spawns a scheduler user task from an already prepared loaded-program descriptor.
///
/// Ownership contract:
/// - on success, scheduler owns `loaded.cr3` lifecycle via task teardown
/// - on failure, this function destroys `loaded.cr3` immediately
///
/// `privileged` is forwarded to [`scheduler::spawn_user_task_owning_code`]; see
/// M6 in `docs/CODE_REVIEW_2026-07-23.md` for what this capability gates.
fn spawn_loaded_program(loaded: LoadedProgram, privileged: bool) -> ExecResult<usize> {
    match scheduler::spawn_user_task_owning_code(
        loaded.entry_rip,
        loaded.user_rsp,
        loaded.cr3,
        privileged,
    ) {
        Ok(task_id) => Ok(task_id),
        Err(e) => {
            crate::logging::logln(
                "loader",
                format_args!(
                    "LOADER: spawn_user_task_owning_code(rip={:#x}, rsp={:#x}, cr3={:#x}) failed: {:?}",
                    loaded.entry_rip, loaded.user_rsp, loaded.cr3, e
                ),
            );
            // Spawn failed before task activation, so only loader-mapped pages
            // exist (code prefix + single bootstrap stack page).
            vmm::destroy_user_address_space_with_page_counts(loaded.cr3, loaded.code_page_count, 1);
            Err(ExecError::SpawnFailed)
        }
    }
}
