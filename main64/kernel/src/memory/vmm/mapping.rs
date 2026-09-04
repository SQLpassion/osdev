use crate::arch::constants::PAGE_SIZE_U64;
use crate::arch::interrupts;
use crate::memory::pmm;

use super::page_table::{
    alloc_frame_phys, alloc_frame_phys_or_panic, entry_ptr, invlpg, page_align_down, pd_index,
    pd_table_addr, pdp_index, pdp_table_addr, phys_to_pfn, pml4_index, pt_for_if_present, pt_index,
    pt_table_addr, read_cr3, table_at, table_entry, table_is_empty, table_zero, walk_levels,
    write_cr3, PageTable, WalkResult, PML4_TABLE_ADDR,
};
use super::{
    classify_user_region, debug_alloc, vmm_logln, UserRegion, TEMP_CLONE_PML4_VA,
    USER_ADDRESS_SPACE_SCAN_END, USER_CODE_BASE, USER_CODE_SIZE, USER_HEAP_BASE, USER_HEAP_END,
    USER_STACK_SIZE, USER_STACK_TOP,
};

/// Error returned by checked mapping operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// Virtual address is already mapped to a different physical frame.
    AlreadyMapped {
        virtual_address: u64,
        current_pfn: u64,
        requested_pfn: u64,
    },

    /// Address is outside configured user mapping regions.
    NotUserRegion { virtual_address: u64 },

    /// Address targets the configured guard page.
    UserGuardPage { virtual_address: u64 },

    /// PMM had no free physical frames for required intermediate page tables.
    OutOfMemory { virtual_address: u64 },

    /// An intermediate level on the path is a present huge-page (2 MiB / 1 GiB)
    /// leaf. The kernel only creates 4 KiB mappings and cannot split a huge page,
    /// so descending into it would corrupt the huge page's backing data. The
    /// `level` field names the offending page-table level ("PDP" or "PD").
    HugePageInPath {
        virtual_address: u64,
        level: &'static str,
    },
}

/// Builds any missing intermediate page tables (PML4/PDP/PD) for `virtual_address`.
///
pub fn populate_page_table_path(virtual_address: u64, user: bool) -> Result<(), MapError> {
    // Level 1: PML4 entry.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);

    if !table_entry(pml4, pml4_idx).present() {
        // Allocate and zero a fresh PDP table.
        let Some(new_table_phys) = alloc_frame_phys() else {
            return Err(MapError::OutOfMemory { virtual_address });
        };

        // SAFETY: `pml4` is a valid PML4 page, `pml4_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            (*entry_ptr(pml4, pml4_idx)).set_mapping(phys_to_pfn(new_table_phys), true, true, user)
        };

        invlpg(pdp_table_addr(virtual_address));
        let new_pdp = table_at(pdp_table_addr(virtual_address));
        table_zero(new_pdp);
        debug_alloc("PML4", pml4_idx, table_entry(pml4, pml4_idx).frame());
    } else if user {
        // Existing path: elevate permissions for user mapping requests.
        // SAFETY: `pml4` is a valid PML4 page, `pml4_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            let e = entry_ptr(pml4, pml4_idx);
            (*e).set_user(true);
            (*e).set_writable(true);
        }
    }

    // Level 2: PDP entry.
    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);

    if !table_entry(pdp, pdp_idx).present() {
        // Allocate and zero a fresh PD table.
        let Some(new_table_phys) = alloc_frame_phys() else {
            return Err(MapError::OutOfMemory { virtual_address });
        };

        // SAFETY: `pdp` is a valid PDP page, `pdp_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            (*entry_ptr(pdp, pdp_idx)).set_mapping(phys_to_pfn(new_table_phys), true, true, user)
        };

        invlpg(pd_table_addr(virtual_address));
        let new_pd = table_at(pd_table_addr(virtual_address));
        table_zero(new_pd);
        debug_alloc("PDP", pdp_idx, table_entry(pdp, pdp_idx).frame());
    } else if table_entry(pdp, pdp_idx).huge() {
        // A present 1 GiB huge-page leaf occupies this slot. Descending would
        // reinterpret the huge page's data frame as a PD and write into it
        // (silent corruption); we cannot split it into 4 KiB pages. Fail loud.
        return Err(MapError::HugePageInPath {
            virtual_address,
            level: "PDP",
        });
    } else if user {
        // Existing path: elevate permissions for user mapping requests.
        // SAFETY: `pdp` is a valid PDP page, `pdp_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            let e = entry_ptr(pdp, pdp_idx);
            (*e).set_user(true);
            (*e).set_writable(true);
        }
    }

    // Level 3: PD entry.
    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);

    if !table_entry(pd, pd_idx).present() {
        // Allocate and zero a fresh PT table.
        let Some(new_table_phys) = alloc_frame_phys() else {
            return Err(MapError::OutOfMemory { virtual_address });
        };
        // SAFETY: `pd` is a valid PD page, `pd_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            (*entry_ptr(pd, pd_idx)).set_mapping(phys_to_pfn(new_table_phys), true, true, user)
        };

        invlpg(pt_table_addr(virtual_address));
        let new_pt = table_at(pt_table_addr(virtual_address));
        table_zero(new_pt);
        debug_alloc("PD", pd_idx, table_entry(pd, pd_idx).frame());
    } else if table_entry(pd, pd_idx).huge() {
        // A present 2 MiB huge-page leaf occupies this slot. Descending would
        // reinterpret the huge page's data frame as a PT and write into it
        // (silent corruption); we cannot split it into 4 KiB pages. Fail loud.
        return Err(MapError::HugePageInPath {
            virtual_address,
            level: "PD",
        });
    } else if user {
        // Existing path: elevate permissions for user mapping requests.
        // SAFETY: `pd` is a valid PD page, `pd_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            let e = entry_ptr(pd, pd_idx);
            (*e).set_user(true);
            (*e).set_writable(true);
        }
    }

    Ok(())
}

/// Bundles the already-resolved tables and indices for one virtual address's
/// PML4/PDP/PD/PT path, as produced by a successful [`walk_levels`] resolution.
///
/// Grouping these together (instead of passing eight raw pointers/indices
/// around individually) keeps [`clear_leaf_and_prune`] a plain 3-argument
/// function and gives the shared-path callers in [`unmap_page_and_prune_pagetable_hierarchy`]
/// and [`reclaim_user_range`] one obvious place to construct it from the
/// pointers they compute (cheaply, via [`table_at`] + the `*_table_addr`
/// helpers) once `walk_levels` confirms the path resolves.
struct ResolvedPath {
    pml4: *mut PageTable,
    pml4_idx: usize,
    pdp: *mut PageTable,
    pdp_idx: usize,
    pd: *mut PageTable,
    pd_idx: usize,
    pt: *mut PageTable,
}

impl ResolvedPath {
    /// Recomputes the table pointers/indices for `virtual_address`.
    ///
    /// Callers must already know the path resolves (e.g. `walk_levels`
    /// returned `WalkResult::Resolved` for this address) -- recomputation
    /// here is pure address arithmetic (`table_at` + the `*_table_addr`
    /// helpers), not a fresh page-table read, so this does not reintroduce
    /// the redundant walk this refactor removes.
    fn for_virtual_address(virtual_address: u64) -> Self {
        Self {
            pml4: table_at(PML4_TABLE_ADDR),
            pml4_idx: pml4_index(virtual_address),
            pdp: table_at(pdp_table_addr(virtual_address)),
            pdp_idx: pdp_index(virtual_address),
            pd: table_at(pd_table_addr(virtual_address)),
            pd_idx: pd_index(virtual_address),
            pt: table_at(pt_table_addr(virtual_address)),
        }
    }
}

/// Clears the leaf PTE for `virtual_address` in an *already-resolved* 4-level
/// `path` and prunes now-empty PD/PDP/PML4 levels bottom-up.
///
/// This is the shared tail end of both [`unmap_page_and_prune_pagetable_hierarchy`]
/// (which resolves the path itself via [`walk_levels`]) and [`reclaim_user_range`]
/// (whose scanning loop has already resolved the same path one level at a time
/// while deciding how far to descend, and passes the tables/indices straight
/// through instead of re-walking from PML4 for every present page -- see
/// issue #58, finding L1).
///
/// Callers must guarantee `path` was built for `virtual_address` and that its
/// PML4/PDP/PD entries are present and non-huge (as [`WalkResult::Resolved`]
/// guarantees).
///
/// If `release_leaf_pfn` is `true`, the leaf PFN is returned to PMM. If
/// `false`, the leaf mapping is only cleared. This helper is used by
/// address-space teardown paths and intentionally does not log warnings when
/// a leaf PFN is not PMM-managed.
fn clear_leaf_and_prune(virtual_address: u64, path: ResolvedPath, release_leaf_pfn: bool) {
    let ResolvedPath {
        pml4,
        pml4_idx,
        pdp,
        pdp_idx,
        pd,
        pd_idx,
        pt,
    } = path;
    let pt_idx = pt_index(virtual_address);

    // Step 1: Clear the leaf PTE.
    // Optionally release the old leaf PFN depending on caller policy:
    // - true  => regular owned user page, return frame to PMM
    // - false => alias/scratch mapping, only remove mapping
    if table_entry(pt, pt_idx).present() {
        let leaf_pfn = table_entry(pt, pt_idx).frame();

        // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
        unsafe { (*entry_ptr(pt, pt_idx)).clear() };
        invlpg(virtual_address);
        if release_leaf_pfn {
            let _ = pmm::with_pmm(|mgr| mgr.release_pfn(leaf_pfn));
        }
    }

    // Step 2: Bottom-up pruning.
    // Only remove a parent-table entry if the child table became empty.
    // This guarantees we never drop shared siblings.
    if !table_is_empty(pt.cast_const()) {
        return;
    }

    let pt_pfn = table_entry(pd, pd_idx).frame();

    // SAFETY: `pd` is a valid PD page, `pd_idx < PT_ENTRIES`, interrupts disabled.
    unsafe { (*entry_ptr(pd, pd_idx)).clear() };
    invlpg(pt_table_addr(virtual_address));
    let _ = pmm::with_pmm(|mgr| mgr.release_pfn(pt_pfn));

    if !table_is_empty(pd.cast_const()) {
        return;
    }

    let pd_pfn = table_entry(pdp, pdp_idx).frame();

    // SAFETY: `pdp` is a valid PDP page, `pdp_idx < PT_ENTRIES`, interrupts disabled.
    unsafe { (*entry_ptr(pdp, pdp_idx)).clear() };
    invlpg(pd_table_addr(virtual_address));
    let _ = pmm::with_pmm(|mgr| mgr.release_pfn(pd_pfn));

    if !table_is_empty(pdp.cast_const()) {
        return;
    }

    let pdp_pfn = table_entry(pml4, pml4_idx).frame();

    // SAFETY: `pml4` is a valid PML4 page, `pml4_idx < PT_ENTRIES`, interrupts disabled.
    unsafe { (*entry_ptr(pml4, pml4_idx)).clear() };
    invlpg(pdp_table_addr(virtual_address));
    let _ = pmm::with_pmm(|mgr| mgr.release_pfn(pdp_pfn));
}

/// Clears one mapped leaf page and prunes empty page-table levels for `virtual_address`.
///
/// This helper is used by address-space teardown paths and intentionally does
/// not log warnings when a leaf PFN is not PMM-managed.
///
/// If `release_leaf_pfn` is `true`, the leaf PFN is returned to PMM.
/// If `false`, the leaf mapping is only cleared.
pub fn unmap_page_and_prune_pagetable_hierarchy(virtual_address: u64, release_leaf_pfn: bool) {
    let virtual_address = page_align_down(virtual_address);

    // Resolve the full 4-level path for `virtual_address` through the shared
    // walk. If any intermediate level is missing (or huge-mapped), there is
    // no normal 4 KiB leaf to clear and therefore nothing to prune.
    let WalkResult::Resolved { .. } = walk_levels(virtual_address) else {
        return;
    };

    // `walk_levels` already confirmed every level is present and non-huge, so
    // recomputing the table pointers/indices here is pure address arithmetic
    // (no additional page-table reads) -- `clear_leaf_and_prune` needs the
    // raw mutable pointers to prune entries, which `WalkResult` intentionally
    // does not carry (see its doc comment).
    let path = ResolvedPath::for_virtual_address(virtual_address);
    clear_leaf_and_prune(virtual_address, path, release_leaf_pfn);
}

/// Maps `virtual_address` to `physical_address` with present + writable flags.
///
/// Returns an error if the VA is already mapped to a different frame.
pub fn try_map_virtual_to_physical(
    virtual_address: u64,
    physical_address: u64,
) -> Result<(), MapError> {
    // Note: single-core, IF-disabled
    // Normalize both addresses to page granularity.
    let virtual_address = page_align_down(virtual_address);
    let physical_address = page_align_down(physical_address);
    let requested_pfn = phys_to_pfn(physical_address);

    // Ensure intermediate levels exist for the target VA.
    populate_page_table_path(virtual_address, false)?;
    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Existing mapping path: only accept if PFN matches requested PFN.
    if table_entry(pt, pt_idx).present() {
        let current_pfn = table_entry(pt, pt_idx).frame();

        if current_pfn != requested_pfn {
            return Err(MapError::AlreadyMapped {
                virtual_address,
                current_pfn,
                requested_pfn,
            });
        }

        return Ok(());
    }

    // Fresh mapping path.
    // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
    unsafe { (*entry_ptr(pt, pt_idx)).set_mapping(requested_pfn, true, true, false) };
    invlpg(virtual_address);
    debug_alloc("PT", pt_idx, table_entry(pt, pt_idx).frame());

    Ok(())
}

/// Maps `virtual_address` to `physical_address` with present + writable flags,
/// and configures the cache to Write-Combining (WC) via PAT1 (PWT=1).
pub fn map_virtual_to_physical_wc(virtual_address: u64, physical_address: u64) {
    // Note: single-core, IF-disabled
    // Thin wrapper that acts like map_virtual_to_physical but sets PWT.
    match try_map_virtual_to_physical(virtual_address, physical_address) {
        Ok(()) => {}
        Err(e) => panic!("VMM: WC mapping failed: {:?}", e),
    }

    // Now set the PWT bit in the leaf entry to select PAT1 (Write-Combining).
    let pt = table_at(super::page_table::pt_table_addr(virtual_address));
    let pt_idx = super::page_table::pt_index(virtual_address);
    // SAFETY: We just successfully mapped it, so it is present.
    unsafe {
        let e = super::page_table::entry_ptr(pt, pt_idx);
        (*e).set_pwt(true); // PWT=1, PCD=0 maps to PAT1 (WC)
    }
    super::page_table::invlpg(virtual_address);
}

/// Maps `virtual_address` to `physical_address` with present + writable flags,
/// and configures the cache to Uncacheable (UC) via PWT=1, PCD=1.
pub fn map_virtual_to_physical_uc(virtual_address: u64, physical_address: u64) {
    // Note: single-core, IF-disabled
    match try_map_virtual_to_physical(virtual_address, physical_address) {
        Ok(()) => {}
        Err(e) => panic!("VMM: UC mapping failed: {:?}", e),
    }

    let pt = table_at(super::page_table::pt_table_addr(virtual_address));
    let pt_idx = super::page_table::pt_index(virtual_address);
    // SAFETY: We just successfully mapped it, so it is present.
    unsafe {
        let e = super::page_table::entry_ptr(pt, pt_idx);
        (*e).set_pcd(true);
        (*e).set_pwt(true);
    }
    super::page_table::invlpg(virtual_address);
}

/// Maps `virtual_address` to `physical_address` with present + writable flags.
///
/// Panics if the VA is already mapped to another frame.
pub fn map_virtual_to_physical(virtual_address: u64, physical_address: u64) {
    // Note: single-core, IF-disabled
    // Thin wrapper: convert checked map errors into a hard panic.
    match try_map_virtual_to_physical(virtual_address, physical_address) {
        Ok(()) => {}
        Err(MapError::AlreadyMapped {
            virtual_address,
            current_pfn,
            requested_pfn,
        }) => {
            panic!(
                "VMM: mapping conflict for VA 0x{:x}: current PFN=0x{:x}, requested PFN=0x{:x}",
                virtual_address, current_pfn, requested_pfn
            );
        }
        Err(MapError::OutOfMemory { virtual_address }) => {
            panic!(
                "VMM: out of physical memory while mapping VA 0x{:x}",
                virtual_address
            );
        }
        Err(MapError::UserGuardPage { virtual_address }) => {
            panic!(
                "VMM: unexpected guard-page map request for VA 0x{:x}",
                virtual_address
            );
        }
        Err(MapError::NotUserRegion { virtual_address }) => {
            panic!(
                "VMM: unexpected non-user map request for VA 0x{:x}",
                virtual_address
            );
        }
        Err(MapError::HugePageInPath {
            virtual_address,
            level,
        }) => {
            panic!(
                "VMM: cannot map VA 0x{:x}: {} level holds a huge page (no 4 KiB split support)",
                virtual_address, level
            );
        }
    }
}

/// Unmaps the given virtual address and invalidates the corresponding TLB entry.
pub fn unmap_virtual_address(virtual_address: u64) {
    // Note: single-core, IF-disabled
    // Operate on page boundary regardless of caller offset.
    let virtual_address = page_align_down(virtual_address);

    // If the hierarchy does not exist, unmap is already satisfied.
    let Some(pt) = pt_for_if_present(virtual_address) else {
        return;
    };

    let pt_idx = pt_index(virtual_address);
    if table_entry(pt, pt_idx).present() {
        // Remove leaf mapping and invalidate stale translation.
        let old_pfn = table_entry(pt, pt_idx).frame();

        // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
        unsafe { (*entry_ptr(pt, pt_idx)).clear() };
        invlpg(virtual_address);

        // Return physical frame ownership to PMM when possible.
        let released = pmm::with_pmm(|mgr| mgr.release_pfn(old_pfn));

        if !released {
            // Best-effort warning for non-PMM-managed mappings.
            vmm_logln(format_args!(
                "VMM: warning: unmapped VA 0x{:x} had non-PMM PFN 0x{:x}",
                virtual_address, old_pfn
            ));
        }
    }
}

/// Clears the given mapping without releasing the mapped PFN back to PMM.
///
/// Intended for temporary virtual mappings to already-owned frames.
pub fn unmap_without_release(virtual_address: u64) {
    // Note: single-core, IF-disabled
    // Keep semantics for the mapped leaf (do not release), but prune and
    // release now-empty table levels so temporary mapping paths do not leak.
    unmap_page_and_prune_pagetable_hierarchy(virtual_address, false);
}

/// Executes `f` while `pml4_phys` is active in CR3, then restores previous state.
///
/// Interrupts are disabled for the whole critical section so timer preemption
/// cannot observe a temporary address-space switch.
pub fn with_address_space<R>(pml4_phys: u64, f: impl FnOnce() -> R) -> R {
    // Preserve interrupt state and block preemption during temporary CR3 switch.
    let interrupts_were_enabled = interrupts::are_enabled();
    if interrupts_were_enabled {
        interrupts::disable();
    }

    // Capture current root so we can restore it unconditionally.
    let previous_cr3 = read_cr3();

    // Switch only when target differs from current root.
    if previous_cr3 != pml4_phys {
        // SAFETY:
        // - This requires `unsafe` because changing CPU address-space state is a privileged operation outside Rust's guarantees.
        // - `pml4_phys` is supplied by trusted kernel code that owns the target root.
        // - Interrupts are disabled for the entire temporary switch.
        unsafe {
            switch_page_directory(pml4_phys);
        }
    }

    // Execute caller work while target address space is active.
    let result = f();

    // Restore original CR3 before leaving critical section.
    if previous_cr3 != pml4_phys {
        // SAFETY:
        // - This requires `unsafe` because changing CPU address-space state is a privileged operation outside Rust's guarantees.
        // - `previous_cr3` was read from the CPU before switching and is valid.
        // - Restoring CR3 under disabled interrupts returns to the original context.
        unsafe {
            switch_page_directory(previous_cr3);
        }
    }

    // Restore interrupt enable state to exactly what caller had.
    if interrupts_were_enabled {
        interrupts::enable();
    }

    result
}

/// Switches to the provided page directory (physical PML4 address).
///
/// # Safety
/// The caller must ensure `pml4_phys` points to a valid, fully initialized
/// PML4 table in physical memory. Switching to an invalid CR3 target can
/// immediately crash the kernel due to page faults/triple fault.
pub unsafe fn switch_page_directory(pml4_phys: u64) {
    // CPU state update.
    write_cr3(pml4_phys);
}

/// Clones the kernel PML4 into a new physical frame for a fresh user address space.
///
/// The returned physical address points to a copied PML4 image with recursive
/// mapping updated to self-reference in entry 511.
///
/// # Why we always clone from the *kernel* PML4
///
/// When called from a user task's syscall (e.g. `Exec`) the CPU's active CR3
/// is the **user task's** address space, which already has pages mapped at
/// `USER_CODE_BASE` (the task's own code).  `PML4_TABLE_ADDR` is a recursively
/// derived virtual address that always reflects the *currently active* PML4.
/// Copying it directly would therefore embed the calling task's user-code and
/// user-stack entries into the clone, causing `AlreadyMapped` errors when the
/// loader later tries to map the new program's pages into the same VAs.
///
/// The fix: temporarily switch to the stored kernel-only PML4 root before
/// performing the copy, then restore the previous CR3 afterwards.  This ensures
/// the clone starts from a clean kernel-only address space with no user-half
/// entries.
///
/// Detailed flow:
/// - Allocate one new physical frame for the clone root table.
/// - Temporarily switch CR3 to the kernel PML4 so `PML4_TABLE_ADDR` resolves
///   against a user-free address space.
/// - Map the new frame at [`TEMP_CLONE_PML4_VA`] and copy one PML4 page.
/// - Update entry 511 in the clone to self-reference (recursive mapping).
/// - Remove the temporary VA mapping.
/// - Restore the previous CR3 and return the clone frame physical address.
///
/// Safety/ownership note:
/// - The returned frame remains allocated and owned by the caller.
/// - `unmap_without_release` is used intentionally so PMM does not free it.
pub fn clone_kernel_pml4_for_user() -> u64 {
    // Step 1: Ensure interrupts are off for the entire CR3-switch window.
    // All operations below touch CPU-visible page-table structures and must
    // not be interrupted by a timer tick that would see an inconsistent CR3.
    let interrupts_were_enabled = interrupts::are_enabled();
    if interrupts_were_enabled {
        interrupts::disable();
    }

    // Step 2: Allocate the new PML4 frame before switching CR3, so that
    // the allocator runs in the context we entered with and cannot race.
    let new_pml4_phys =
        alloc_frame_phys_or_panic("VMM: out of physical memory while cloning user PML4");

    // Step 3: Temporarily switch to the kernel PML4.
    // This makes PML4_TABLE_ADDR (recursive mapping VA) resolve to the
    // kernel-only address space, free of any user-space code/stack entries.
    let previous_cr3 = read_cr3();
    let kernel_pml4 = super::get_pml4_address();
    if previous_cr3 != kernel_pml4 {
        // interrupts are disabled above; `kernel_pml4` is the validated stable
        // root from VMM init.  This is a transient switch restored in Step 6.
        write_cr3(kernel_pml4);
    }

    // Step 4: Map the clone frame at the scratch VA, then copy the kernel PML4.
    unmap_without_release(TEMP_CLONE_PML4_VA);
    map_virtual_to_physical(TEMP_CLONE_PML4_VA, new_pml4_phys);

    // SAFETY:
    // - CR3 is now the kernel PML4, so PML4_TABLE_ADDR resolves correctly.
    // - TEMP_CLONE_PML4_VA is mapped to `new_pml4_phys` just above.
    // - Source and destination are disjoint, each exactly one page long.
    // - Interrupts are disabled; no concurrent CR3 change can occur.
    unsafe {
        core::ptr::copy_nonoverlapping(
            PML4_TABLE_ADDR as *const u8,
            TEMP_CLONE_PML4_VA as *mut u8,
            PAGE_SIZE_U64 as usize,
        );
    }

    // Step 5: Rebind recursive slot 511 inside the clone.
    //
    // After the raw memcpy, entry 511 in the clone still holds the kernel PML4
    // physical address (copied from the kernel PML4). Overwrite it so that the
    // clone's recursive window points to the clone frame itself, enabling
    // independent page-table walks once this CR3 is activated.
    let clone_pml4 = table_at(TEMP_CLONE_PML4_VA);
    // SAFETY:
    // - `clone_pml4` is a valid PML4 page mapped at TEMP_CLONE_PML4_VA.
    // - 511 < PT_ENTRIES.
    // - Interrupts disabled throughout; no concurrent access.
    unsafe {
        (*entry_ptr(clone_pml4, 511)).set_mapping(phys_to_pfn(new_pml4_phys), true, true, false)
    };

    // Drop the transient scratch entry from the clone.
    //
    // `TEMP_CLONE_PML4_VA` lives in its own PML4 slot, so the memcpy above also
    // copied the kernel's scratch mapping (the slot pointing at the scratch
    // PDPT) into the clone. The `unmap_without_release` below prunes and frees
    // that scratch hierarchy on the *kernel* side, which would leave the clone's
    // copied entry dangling at a soon-to-be-reused frame. Nothing ever walks
    // this slot in a user address space, but clear it so the clone carries no
    // stale mapping.
    let scratch_pml4_idx = ((TEMP_CLONE_PML4_VA >> 39) & 0x1FF) as usize;
    // SAFETY:
    // - `clone_pml4` is a valid PML4 page; `scratch_pml4_idx < PT_ENTRIES`.
    // - Interrupts disabled throughout; no concurrent access.
    unsafe { (*entry_ptr(clone_pml4, scratch_pml4_idx)).clear() };

    unmap_without_release(TEMP_CLONE_PML4_VA);

    // Step 6: Restore the previous CR3 before re-enabling interrupts.
    if previous_cr3 != kernel_pml4 {
        // `previous_cr3` was read from the CPU before the switch and is valid.
        write_cr3(previous_cr3);
    }

    if interrupts_were_enabled {
        interrupts::enable();
    }

    new_pml4_phys
}

/// Destroys a user address space rooted at `pml4_phys`.
///
/// Teardown semantics:
/// - unmaps user-code, user-stack, and user-heap ranges,
/// - reclaims any other present user mapping outside those three windows
///   (e.g. a future `mmap`-created region) via a generic catch-all scan,
/// - releases every mapped PMM-managed leaf frame it unmaps through the
///   refcounted `PhysicalMemoryManager::release_pfn`, which only actually
///   frees a frame once its last owner releases it — so frames shared with
///   another mapping (via `inc_refcount` at the site that created the alias)
///   safely survive this call and are freed later, when their other owner
///   releases them,
/// - prunes and releases now-empty PT/PD/PDP pages,
/// - releases the root PML4 frame itself.
pub fn destroy_user_address_space(pml4_phys: u64, mmio_skip_release: &[(u64, usize)]) {
    // Note: single-core, IF-disabled
    destroy_user_address_space_with_page_counts(
        pml4_phys,
        (USER_CODE_SIZE / PAGE_SIZE_U64) as usize,
        (USER_STACK_SIZE / PAGE_SIZE_U64) as usize,
        mmio_skip_release,
    );
}

/// Destroys a user address space with explicit mapped-page counts.
///
/// ## What this function does
/// 1. Temporarily activates `pml4_phys` as the current CR3 (via [`with_address_space`])
///    so that recursive page-table walk addresses resolve against the correct hierarchy.
/// 2. Unmaps every mapped page in the known `USER_CODE`, `USER_STACK`, and
///    `USER_HEAP` windows, pruning now-empty PT/PD/PDP frames as it goes.
/// 3. Scans the remainder of the user PML4 slot range
///    (`[USER_CODE_BASE, USER_ADDRESS_SPACE_SCAN_END)`) for any other present
///    mapping and reclaims it too, so a region outside those three fixed
///    windows (e.g. a future per-process `mmap` allocation) cannot be
///    silently leaked.
/// 4. Releases the root PML4 frame back to the PMM.
/// 5. Restores the previous CR3 before returning.
///
/// Every leaf frame unmapped along the way goes through
/// `PhysicalMemoryManager::release_pfn`, which is refcounted: a frame with
/// more than one owner (bumped via `inc_refcount` at the site that created
/// the extra mapping — e.g. a code page deliberately aliased over another
/// mapping) merely has its count decremented here and stays allocated until
/// its other owner(s) release it too. This is what replaced the old
/// `release_user_code_pfns` boolean policy: callers no longer choose whether
/// to release a window's frames — the refcount always decides.
///
/// `stack_page_count_from_top` is interpreted as a contiguous window growing
/// downward from [`USER_STACK_TOP`], matching how user stacks are allocated.
/// Count values are clamped to configured region capacities.
///
/// `mmio_skip_release` lists `(page_va_start, num_pages)` ranges — captured from
/// the exiting task's `DriverCaps::allocations` before that block was freed —
/// that map a device MMIO BAR window (`MmioAllocKind::Mmio`) rather than
/// PMM-owned RAM. The catch-all scan in step 3 still unmaps every page in
/// these ranges (so nothing stays mapped after teardown), but skips
/// `release_pfn` for them, since that physical address was never owned by the
/// PMM and must not be added to its free list. Pass `&[]` for a task that
/// never held an MMIO grant.
///
/// ## What this function does NOT do
/// - It does not touch any kernel-half mappings (PML4 entries 256 and above), nor
///   PML4 slot 0 (the low-memory identity map). Those are shared with every other
///   address space and must remain intact; see [`USER_ADDRESS_SPACE_SCAN_END`].
///
/// ## Caller constraints
/// - Must NOT be called with `pml4_phys` equal to the kernel CR3 that has no
///   corresponding user address space — doing so would unmap the user windows
///   inside the kernel page tables, corrupting all future user tasks.
/// - Interrupts are disabled for the duration of the CR3 switch (handled internally
///   by [`with_address_space`]).
pub fn destroy_user_address_space_with_page_counts(
    pml4_phys: u64,
    code_page_count: usize,
    stack_page_count_from_top: usize,
    mmio_skip_release: &[(u64, usize)],
) {
    // Note: single-core, IF-disabled
    // Always operate on a canonical page-aligned root frame.
    let pml4_phys = page_align_down(pml4_phys);

    // A zero root is treated as "no address space" and is therefore a no-op.
    if pml4_phys == 0 {
        return;
    }

    // Clamp caller-provided counts to configured region capacities.
    let max_code_pages = (USER_CODE_SIZE / PAGE_SIZE_U64) as usize;
    let max_stack_pages = (USER_STACK_SIZE / PAGE_SIZE_U64) as usize;
    let code_pages = code_page_count.min(max_code_pages);
    let stack_pages = stack_page_count_from_top.min(max_stack_pages);

    // Teardown must run while the target CR3 is active so recursive-table
    // helper addresses resolve to the correct hierarchy.
    with_address_space(pml4_phys, || {
        // Step 1: Drop user-code mappings for the known mapped prefix. The refcounted
        // `release_pfn` decides whether a code frame is actually freed here or merely
        // has its count decremented (when another mapping still owns it).
        let mut va = USER_CODE_BASE;
        for _ in 0..code_pages {
            unmap_page_and_prune_pagetable_hierarchy(va, true);
            va += PAGE_SIZE_U64;
        }

        // Step 2: Drop mapped user-stack pages in the top-down stack window.
        let mut stack_va = USER_STACK_TOP - (stack_pages as u64 * PAGE_SIZE_U64);
        while stack_va < USER_STACK_TOP {
            unmap_page_and_prune_pagetable_hierarchy(stack_va, true);
            stack_va += PAGE_SIZE_U64;
        }

        // Step 3: Clear and release all mapped pages in the user-mode heap region.
        unmap_user_heap_region();

        // Step 4: Catch-all — reclaim any other present user mapping outside the
        // three windows above (e.g. a future mmap-created region, or this task's
        // MMIO/DMA window). Code/Stack/Heap were already cleared, so in the common
        // case this scan only has the MMIO/DMA window left to walk; it exists so
        // nothing in the user PML4 slot range can be silently leaked at teardown.
        reclaim_user_range(USER_CODE_BASE, USER_ADDRESS_SPACE_SCAN_END, mmio_skip_release);
    });

    // Finally release the root PML4 frame itself after its hierarchy has been pruned.
    let released = pmm::with_pmm(|mgr| mgr.release_pfn(phys_to_pfn(pml4_phys)));

    if !released {
        // Best-effort diagnostics: teardown already cleared mappings, but PMM
        // ownership metadata was not in the expected state for this root PFN.
        vmm_logln(format_args!(
            "VMM: warning: destroy_user_address_space could not release root PFN 0x{:x}",
            phys_to_pfn(pml4_phys)
        ));
    }
}

/// Unmaps all mapped pages in the user heap region [USER_HEAP_BASE..USER_HEAP_END).
///
/// This traverses intermediate page table directories to efficiently skip
/// unmapped sub-regions and prunes hierarchy frames as they become empty.
pub fn unmap_user_heap_region() {
    reclaim_user_range(USER_HEAP_BASE, USER_HEAP_END, &[]);
}

/// Reclaims every present user leaf mapping in `[scan_start, scan_end)`.
///
/// Walks the 4-level page-table hierarchy via the shared [`walk_levels`],
/// skipping non-present (or huge-mapped) sub-trees at the largest granularity
/// possible (512 GiB / 1 GiB / 2 MiB jumps) so large unmapped gaps are cheap
/// to pass over, and releases each present leaf frame via the refcounted
/// `PhysicalMemoryManager::release_pfn` while pruning now-empty PT/PD/PDP
/// levels — the same technique `unmap_user_heap_region` has always used for
/// the heap window, generalized to arbitrary bounds.
///
/// For each present page this loop has *already* resolved the PML4/PDP/PD
/// path while deciding not to skip past it, so the present-page case feeds
/// those already-resolved tables/indices straight into
/// [`clear_leaf_and_prune`] instead of re-entering the full 4-level walk per
/// page through [`unmap_page_and_prune_pagetable_hierarchy`] — see issue #58,
/// finding L1 (this used to walk the hierarchy twice per reclaimed page).
///
/// Note: user mappings are only ever created as 4 KiB pages (see
/// [`map_user_page`]), so a huge PDP/PD entry is never expected inside the
/// user scan range in practice; treating it the same as "not present" here
/// (skip past it at its native granularity) keeps this loop's bail
/// conditions consistent with every other walk in the VMM instead of
/// silently misinterpreting a huge page's data frame as a child table
/// address, which the pre-#58 version of this loop did not guard against.
///
/// Callers must ensure the target address space is already active (e.g. via
/// [`with_address_space`]) so recursive-mapping helper addresses resolve
/// correctly, and must only pass bounds known not to overlap
/// address-space-shared infrastructure (PML4 slot 0's identity map, or the
/// higher-half kernel slots) — see [`USER_ADDRESS_SPACE_SCAN_END`].
///
/// `mmio_skip_release` lists `(page_va_start, num_pages)` ranges that must be
/// unmapped but never passed to `release_pfn` — see the parameter doc on
/// [`destroy_user_address_space_with_page_counts`].
fn reclaim_user_range(scan_start: u64, scan_end: u64, mmio_skip_release: &[(u64, usize)]) {
    let mut va = scan_start;
    while va < scan_end {
        match walk_levels(va) {
            WalkResult::Pml4Missing => {
                // PML4 entries cover 512 GiB of virtual address space.
                va = (va + 0x80_0000_0000) & !(0x80_0000_0000 - 1);
            }
            WalkResult::PdpMissing { .. } | WalkResult::PdpHuge { .. } => {
                // PDPT entries cover 1 GiB of virtual address space.
                va = (va + 0x4000_0000) & !(0x4000_0000 - 1);
            }
            WalkResult::PdMissing { .. } | WalkResult::PdHuge { .. } => {
                // PD entries cover 2 MiB of virtual address space.
                va = (va + 0x20_0000) & !(0x20_0000 - 1);
            }
            WalkResult::Resolved { .. } => {
                // Page table level exists; reuse the already-resolved path to
                // clear and prune this one page, then advance by page size.
                let path = ResolvedPath::for_virtual_address(va);
                let release_pfn = !mmio_skip_release.iter().any(|&(base, pages)| {
                    let end = base + (pages as u64) * PAGE_SIZE_U64;
                    va >= base && va < end
                });
                clear_leaf_and_prune(va, path, release_pfn);
                va += PAGE_SIZE_U64;
            }
        }
    }
}

/// Maps one user virtual page to `pfn` using user-accessible permissions.
///
/// `virtual_address` must be within configured user code/stack regions and
/// must not target the configured guard page.
///
/// This function mutates page tables via recursive mapping and therefore
/// requires a stable active address space while it runs. Callers must execute
/// it only inside [`with_address_space`] (or an equivalent critical section)
/// that:
/// - disables interrupts for the full duration, and
/// - guarantees `CR3` does not change until the function returns.
///
/// If this precondition is violated, a context switch can switch to a different
/// `CR3` while recursive addresses are being resolved, which can race and write
/// into the wrong page-table hierarchy. This is not `unsafe fn` (no raw pointer
/// or memory-layout invariant is handed to the caller to uphold), but the
/// precondition is still checked in debug builds via a cheap `debug_assert!` in
/// the shared [`map_user_leaf`] body — see issue #51.
pub fn map_user_page(virtual_address: u64, pfn: u64, writable: bool) -> Result<(), MapError> {
    // Note: single-core, IF-disabled
    // Normalize to 4 KiB page granularity; callers may pass any address
    // within the target page.
    let virtual_address = page_align_down(virtual_address);

    // Enforce user-window policy before touching page tables.
    // Derive the NX policy from the region:
    //   - CODE  → no_execute = false  (pages must be executable)
    //   - STACK → no_execute = true   (pages must not be executable; prevents stack injection)
    //   - HEAP  → no_execute = true
    // EFER.NXE is enabled early in the kernel (arch::msr::enable_no_execute, called
    // from KernelMain); without it bit 63 is reserved and faults on real hardware.
    //
    // Note: the CODE case predates per-segment ELF permissions and is kept only
    // for callers that map a single flat code page (none remain in-tree since
    // the ELF loader migration, but the fallback stays conservative). ELF
    // segment pages that need write-without-execute or read-only-without-write
    // must go through [`map_user_code_page`] instead, which takes an explicit
    // per-segment `writable`/`executable` pair derived from `p_flags`.
    let no_execute = match classify_user_region(virtual_address) {
        Some(UserRegion::Code) => false,
        Some(UserRegion::Stack) => true,
        Some(UserRegion::Heap) => true,
        Some(UserRegion::Mmio) => true,
        Some(UserRegion::Guard) => {
            return Err(MapError::UserGuardPage { virtual_address });
        }
        None => {
            return Err(MapError::NotUserRegion { virtual_address });
        }
    };

    map_user_leaf(virtual_address, pfn, writable, no_execute)
}

/// Maps one user virtual page inside the code window with explicit per-segment
/// permissions derived from an ELF `PT_LOAD` segment's `p_flags`.
///
/// Unlike [`map_user_page`], which derives a single fixed NX policy for the
/// whole code window, this takes `writable`/`executable` explicitly so the
/// ELF loader can give a R-X `.text` segment and a RW- `.data`/`.bss` segment
/// different permissions within the same code window.
///
/// Same precondition as [`map_user_page`]: callers must run this only inside
/// [`with_address_space`] (or an equivalent critical section) with interrupts
/// disabled for the full duration and a stable `CR3` (checked via
/// `debug_assert!` in the shared [`map_user_leaf`] body — see issue #51).
pub fn map_user_code_page(
    virtual_address: u64,
    pfn: u64,
    writable: bool,
    executable: bool,
) -> Result<(), MapError> {
    // Note: single-core, IF-disabled
    let virtual_address = page_align_down(virtual_address);

    match classify_user_region(virtual_address) {
        Some(UserRegion::Code) => {}
        Some(UserRegion::Guard) => {
            return Err(MapError::UserGuardPage { virtual_address });
        }
        _ => {
            return Err(MapError::NotUserRegion { virtual_address });
        }
    }

    map_user_leaf(virtual_address, pfn, writable, !executable)
}

/// Maps one user virtual page inside the MMIO window with PCD (cache disable),
/// writable, user, and no-execute permissions for device hardware registers.
///
/// Preconditions: callers must run this only inside a critical section with
/// interrupts disabled for the full duration and a stable CR3.
pub fn map_user_mmio_page(virtual_address: u64, pfn: u64) -> Result<(), MapError> {
    // Note: single-core, IF-disabled
    let virtual_address = page_align_down(virtual_address);

    match classify_user_region(virtual_address) {
        Some(UserRegion::Mmio) => {}
        Some(UserRegion::Guard) => {
            return Err(MapError::UserGuardPage { virtual_address });
        }
        _ => {
            return Err(MapError::NotUserRegion { virtual_address });
        }
    }

    map_user_mmio_leaf(virtual_address, pfn)
}

/// Installs (or idempotently updates) an MMIO user leaf mapping with
/// present, writable, user, no_execute, PCD, and PWT flags set (strongly uncacheable).
fn map_user_mmio_leaf(virtual_address: u64, pfn: u64) -> Result<(), MapError> {
    // Note: single-core, IF-disabled
    debug_assert!(
        !interrupts::are_enabled(),
        "map_user_mmio_leaf: must run with interrupts disabled; \
         see map_user_mmio_page preconditions"
    );

    // Step 1: Ensure all intermediate page table levels (PML4, PDP, PD) exist and are marked user-accessible.
    populate_page_table_path(virtual_address, true)?;
    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Step 2: Handle idempotent remap if the leaf is already present.
    if table_entry(pt, pt_idx).present() {
        let current_pfn = table_entry(pt, pt_idx).frame();

        if current_pfn != pfn {
            return Err(MapError::AlreadyMapped {
                virtual_address,
                current_pfn,
                requested_pfn: pfn,
            });
        }

        // SAFETY:
        // - `pt` is a valid PT page mapped into the recursive paging window.
        // - `pt_idx < PT_ENTRIES` (0..512).
        // - Interrupts are disabled on this core and CR3 is stable.
        unsafe {
            let e = entry_ptr(pt, pt_idx);
            (*e).set_writable(true);
            (*e).set_user(true);
            (*e).set_no_execute(true);
            (*e).set_pcd(true);
            (*e).set_pwt(true);
        }

        invlpg(virtual_address);
        return Ok(());
    }

    // Step 3: Fresh mapping path for previously non-present leaf.
    // SAFETY:
    // - `pt` is a valid PT page mapped in the recursive window.
    // - `pt_idx < PT_ENTRIES`.
    // - Interrupts are disabled and CR3 is stable.
    unsafe {
        let e = entry_ptr(pt, pt_idx);
        (*e).set_mapping(pfn, true, true, true);
        (*e).set_no_execute(true);
        (*e).set_pcd(true);
        (*e).set_pwt(true);
    }

    // Invalidate stale translation for this VA in current TLB context.
    invlpg(virtual_address);

    Ok(())
}

/// Shared leaf-mapping body behind [`map_user_page`] and [`map_user_code_page`]:
/// installs (or idempotently updates) a page-aligned user leaf mapping with the
/// given writable/no-execute flags once the caller has already validated the
/// target region.
fn map_user_leaf(
    virtual_address: u64,
    pfn: u64,
    writable: bool,
    no_execute: bool,
) -> Result<(), MapError> {
    // Note: single-core, IF-disabled
    //
    // Debug-only precondition guard (issue #51): this function is deliberately a
    // safe `fn`, not `unsafe fn`, because no raw pointer/layout invariant is
    // handed to the caller — but it still relies on recursive-mapping addresses
    // staying valid across every page-table write below, which only holds if
    // the active address space cannot change mid-call. `with_address_space`
    // (the only sanctioned entry point) always disables interrupts for its
    // whole critical section, so "interrupts enabled here" is a cheap, reliable
    // proxy for "precondition violated" without needing to track CR3 stability
    // directly. This does nothing in release builds.
    debug_assert!(
        !interrupts::are_enabled(),
        "map_user_leaf: must run with interrupts disabled (inside with_address_space); \
         see map_user_page/map_user_code_page preconditions"
    );

    // Ensure all intermediate levels exist and are marked user-accessible.
    populate_page_table_path(virtual_address, true)?;
    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Existing mapping: allow idempotent "same PFN, permission update".
    // Reject remap attempts to a different PFN to avoid silent alias changes.
    if table_entry(pt, pt_idx).present() {
        let current_pfn = table_entry(pt, pt_idx).frame();

        if current_pfn != pfn {
            return Err(MapError::AlreadyMapped {
                virtual_address,
                current_pfn,
                requested_pfn: pfn,
            });
        }

        // Keep `present` + physical frame, update writable, user, and NX flags.
        // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
        unsafe {
            let e = entry_ptr(pt, pt_idx);
            (*e).set_writable(writable);
            (*e).set_user(true);
            (*e).set_no_execute(no_execute);
        }

        // A permission change (e.g. writable → read-only, or adding NX) is not visible
        // to the processor until the stale TLB entry for this VA is evicted.
        // Without invalidation the CPU may keep using the old cached translation.
        invlpg(virtual_address);

        return Ok(());
    }

    // Fresh mapping path for previously non-present leaf.
    // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
    unsafe {
        let e = entry_ptr(pt, pt_idx);
        (*e).set_mapping(pfn, true, writable, true);
        (*e).set_no_execute(no_execute);
    }

    // Invalidate stale translation for this VA in current TLB context.
    invlpg(virtual_address);

    Ok(())
}

/// Configures existing page table entries covering the given virtual address range to use
/// Write-Combining (WC) via PAT1 (PWT=1, PCD=0).
///
/// This correctly handles both 4 KiB and 2 MiB / 1 GiB huge pages by stamping the PWT/PCD
/// bits at the appropriate leaf level, modifying existing mappings (such as UEFI GOP
/// mappings). PCD is stamped *clear* rather than left alone, so a range an earlier pass
/// (or the firmware) mapped uncacheable really ends up write-combining and not UC (#63).
pub fn configure_wc_mapping(start_va: u64, size: u64) {
    configure_cache_bits(start_va, size, true, false);
}

/// Configures existing page table entries covering the given virtual address range to be
/// strongly uncacheable (PWT=1, PCD=1 → PAT3, which is UC in the default PAT) — the
/// counterpart to [`configure_wc_mapping`], for device MMIO that must never be cached.
/// Deliberately the same memory type [`map_virtual_to_physical_uc`] installs, so a caller
/// that creates the mapping on one boot and re-types an inherited one on the next ends up
/// with identical caching either way. (`direct_map::map_4k_uc_page` uses PCD=1/PWT=0 → UC-
/// instead; both are uncacheable here, since KAOS programs no MTRRs for UC- to be weakened
/// by, but strong UC is the safer default for a device aperture.)
///
/// Unlike [`map_virtual_to_physical_uc`], which *creates* a mapping, this fixes up the
/// memory type of whatever mapping already exists, at whatever granularity it exists — a
/// 4 KiB leaf the kernel made itself, or an inherited/kernel-built 2 MiB / 1 GiB huge page.
/// A caller that only skips work when `is_va_mapped` reports the range as mapped otherwise
/// has no way to tell a correctly-uncacheable mapping from a write-back one: the #63
/// kernel-owned table maps `EfiReservedMemoryType` and `EFI_MEMORY_RUNTIME` regions
/// write-back (design doc §4 prescribes no cacheability for those), so a device aperture a
/// firmware happens to describe as Reserved rather than `EfiMemoryMappedIO` would end up
/// cached (#63 B4).
///
/// Note the granularity caveat: re-stamping a huge leaf changes the memory type of the
/// whole 2 MiB / 1 GiB it covers. For a device aperture that is the intent — such a page
/// covers device space, not RAM — and it is the same trade-off `configure_wc_mapping`
/// already makes for the framebuffer.
pub fn configure_uc_mapping(start_va: u64, size: u64) {
    configure_cache_bits(start_va, size, true, true);
}

/// Shared walk behind [`configure_wc_mapping`] / [`configure_uc_mapping`]: stamps `pwt`/
/// `pcd` onto every existing leaf covering `[start_va, start_va + size)`, at whatever
/// granularity that leaf has, and invalidates each touched translation. Non-present levels
/// are skipped rather than created — this only ever *re-types* mappings that already exist.
fn configure_cache_bits(start_va: u64, size: u64, pwt: bool, pcd: bool) {
    // Note: single-core, IF-disabled
    use super::page_table::*;

    let mut addr = page_align_down(start_va);
    let end = page_align_down(start_va + size + PAGE_SIZE_U64 - 1);

    while addr < end {
        // Dispatch on the shared walk (#58) to decide which level (if any) holds
        // the mapping for `addr`, then apply the PWT/PCD bits at exactly that
        // level. Missing entries at any level are skipped one page at a
        // time (matching this function's original per-call behavior);
        // that is a smaller skip than `reclaim_user_range`'s jump-by-region
        // logic, but WC/UC ranges (framebuffer/MMIO) are typically small
        // contiguous windows, so this was never a hot path worth widening
        // here, and preserving it avoids an unrelated behavior change.
        match walk_levels(addr) {
            WalkResult::Pml4Missing
            | WalkResult::PdpMissing { .. }
            | WalkResult::PdMissing { .. } => {
                addr += PAGE_SIZE_U64;
            }
            WalkResult::PdpHuge { .. } => {
                let pdp = table_at(pdp_table_addr(addr));
                let pdp_idx = pdp_index(addr);
                // SAFETY: Modifying cache bits of mapped memory in Ring 0 is safe.
                unsafe {
                    let e = entry_ptr(pdp, pdp_idx);
                    (*e).set_pwt(pwt);
                    (*e).set_pcd(pcd);
                }
                invlpg(addr);
                addr = (addr + 0x4000_0000) & !0x3FFF_FFFF; // Advance by 1GiB
            }
            WalkResult::PdHuge { .. } => {
                let pd = table_at(pd_table_addr(addr));
                let pd_idx = pd_index(addr);
                // SAFETY: Modifying cache bits of mapped memory in Ring 0 is safe.
                unsafe {
                    let e = entry_ptr(pd, pd_idx);
                    (*e).set_pwt(pwt);
                    (*e).set_pcd(pcd);
                }
                invlpg(addr);
                addr = (addr + 0x20_0000) & !0x1F_FFFF; // Advance by 2MiB
            }
            WalkResult::Resolved { pt, .. } => {
                let pt_idx = pt_index(addr);
                if table_entry(pt, pt_idx).present() {
                    // SAFETY: Modifying cache bits of mapped memory in Ring 0 is safe.
                    unsafe {
                        let e = entry_ptr(pt, pt_idx);
                        (*e).set_pwt(pwt);
                        (*e).set_pcd(pcd);
                    }
                    invlpg(addr);
                }
                addr += PAGE_SIZE_U64;
            }
        }
    }
}
