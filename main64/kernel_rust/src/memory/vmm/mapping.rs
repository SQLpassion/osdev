use crate::arch::constants::PAGE_SIZE_U64;
use crate::arch::interrupts;
use crate::memory::pmm;

use super::page_table::{
    alloc_frame_phys, alloc_frame_phys_or_panic, entry_ptr, invlpg, page_align_down, phys_to_pfn,
    pml4_index, pdp_index, pd_index, pt_index, read_cr3, write_cr3, table_at, table_entry,
    table_is_empty, table_zero, pt_for_if_present, pdp_table_addr, pd_table_addr, pt_table_addr,
    PML4_TABLE_ADDR,
};
use super::{
    classify_user_region, debug_alloc, vmm_logln, UserRegion, USER_CODE_BASE,
    USER_CODE_SIZE, USER_STACK_TOP, USER_STACK_SIZE, USER_HEAP_BASE,
    USER_HEAP_END, TEMP_CLONE_PML4_VA,
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

/// Clears one mapped leaf page and prunes empty page-table levels for `virtual_address`.
///
/// This helper is used by address-space teardown paths and intentionally does
/// not log warnings when a leaf PFN is not PMM-managed.
///
/// If `release_leaf_pfn` is `true`, the leaf PFN is returned to PMM.
/// If `false`, the leaf mapping is only cleared.
pub fn unmap_page_and_prune_pagetable_hierarchy(virtual_address: u64, release_leaf_pfn: bool) {
    let virtual_address = page_align_down(virtual_address);

    // Step 1: Resolve the full 4-level path for `virtual_address`.
    // If any intermediate level is missing (or huge-mapped), there is no
    // normal 4KiB leaf to clear and therefore nothing to prune.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = table_entry(pml4, pml4_idx);
    if !pml4e.present() || pml4e.huge() {
        return;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = table_entry(pdp, pdp_idx);
    if !pdpe.present() || pdpe.huge() {
        return;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = table_entry(pd, pd_idx);
    if !pde.present() || pde.huge() {
        return;
    }

    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Step 2: Clear the leaf PTE.
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

    // Step 3: Bottom-up pruning.
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

/// Maps `virtual_address` to `physical_address` with present + writable flags.
///
/// Returns an error if the VA is already mapped to a different frame.
pub fn try_map_virtual_to_physical(
    virtual_address: u64,
    physical_address: u64,
) -> Result<(), MapError> {
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

/// Maps `virtual_address` to `physical_address` with present + writable flags.
///
/// Panics if the VA is already mapped to another frame.
pub fn map_virtual_to_physical(virtual_address: u64, physical_address: u64) {
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
    }
}

/// Unmaps the given virtual address and invalidates the corresponding TLB entry.
pub fn unmap_virtual_address(virtual_address: u64) {
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

/// Clones the active kernel PML4 into a new physical frame for a user address space.
///
/// The returned physical address points to a copied PML4 image with recursive
/// mapping updated to self-reference in entry 511.
///
/// Detailed flow:
/// - Allocate one new physical frame that will hold the clone root table.
/// - Temporarily map that frame at [`TEMP_CLONE_PML4_VA`].
/// - Copy the currently active PML4 page (`PML4_TABLE_ADDR`) byte-for-byte
///   into the temporary mapping. This preserves kernel-half mappings.
/// - Update entry 511 inside the clone so its recursive mapping points to the
///   clone frame itself (not the original kernel PML4).
/// - Remove the temporary VA mapping and return the clone frame physical address.
///
/// Why not write directly via physical address:
/// - Rust code executes in virtual-address context; writing to arbitrary physical
///   addresses requires either identity mapping assumptions or a temporary map.
/// - Using one fixed scratch VA is explicit, deterministic, and avoids keeping
///   permanent helper mappings around.
///
/// Safety/ownership note:
/// - The returned frame remains allocated and owned by the caller.
/// - `unmap_without_release` is used intentionally so PMM does not free it.
pub fn clone_kernel_pml4_for_user() -> u64 {
    let new_pml4_phys =
        alloc_frame_phys_or_panic("VMM: out of physical memory while cloning user PML4");

    // Reuse one temporary VA for clone operations.
    unmap_without_release(TEMP_CLONE_PML4_VA);
    map_virtual_to_physical(TEMP_CLONE_PML4_VA, new_pml4_phys);

    // SAFETY:
    // - This requires `unsafe` because raw memory copy operations require manually proving non-overlap and valid ranges.
    // - Source is the current recursively mapped kernel PML4 page.
    // - Destination is a freshly allocated page mapped at TEMP_CLONE_PML4_VA.
    // - Regions are disjoint and exactly one page long.
    unsafe {
        core::ptr::copy_nonoverlapping(
            PML4_TABLE_ADDR as *const u8,
            TEMP_CLONE_PML4_VA as *mut u8,
            PAGE_SIZE_U64 as usize,
        );
    }

    // Rebind recursive slot 511 inside the clone to point to the clone itself.
    //
    // Background:
    // - In this kernel, PML4 entry 511 is used for recursive page-table mapping.
    // - After the raw memcpy above, entry 511 in the clone still points to the
    //   original kernel PML4 frame (copied value), which is wrong for an
    //   independent address-space root.
    //
    // What these lines do:
    // 1) Interpret the temporary clone mapping as a mutable PML4 table.
    // 2) Overwrite entry 511 so it maps `new_pml4_phys` (the clone frame).
    //
    // Result:
    // - Recursive virtual windows (PML4/PDP/PD/PT helper addresses) operate on
    //   the cloned hierarchy once this CR3 is activated, not on the kernel root.
    let clone_pml4 = table_at(TEMP_CLONE_PML4_VA);
    // SAFETY: `clone_pml4` is a valid PML4 page (freshly mapped scratch at TEMP_CLONE_PML4_VA),
    // `511 < PT_ENTRIES`, interrupts disabled for the duration of the scratch mapping.
    unsafe {
        (*entry_ptr(clone_pml4, 511)).set_mapping(phys_to_pfn(new_pml4_phys), true, true, false)
    };

    unmap_without_release(TEMP_CLONE_PML4_VA);

    new_pml4_phys
}

/// Destroys a user address space rooted at `pml4_phys`.
///
/// Teardown semantics:
/// - unmaps user-code and user-stack ranges,
/// - releases mapped PMM-managed leaf frames in stack range,
/// - keeps code-range leaf PFNs reserved (alias-safe default),
/// - prunes and releases now-empty PT/PD/PDP pages,
/// - releases the root PML4 frame itself.
pub fn destroy_user_address_space(pml4_phys: u64) {
    // Keep legacy default: do not release USER_CODE PFNs (alias-safe mode).
    destroy_user_address_space_with_options(pml4_phys, false);
}

/// Destroys a user address space rooted at `pml4_phys` with explicit code-page policy.
///
/// ## What this function does
/// 1. Temporarily activates `pml4_phys` as the current CR3 (via [`with_address_space`])
///    so that recursive page-table walk addresses resolve against the correct hierarchy.
/// 2. Unmaps every page in `[USER_CODE_BASE, USER_CODE_END)` and
///    `[USER_STACK_BASE, USER_STACK_TOP)`, pruning now-empty PT/PD/PDP frames as it
///    goes.
/// 3. Releases the root PML4 frame back to the PMM.
/// 4. Restores the previous CR3 before returning.
///
/// ## What this function does NOT do
/// - It does not touch any kernel-half mappings (PML4 entries 256 and above). Those
///   are shared with every other address space and must remain intact.
/// - It does not handle regions outside `USER_CODE` and `USER_STACK`; any other
///   user mappings that exist would be silently leaked.
///
/// ## Caller constraints
/// - Must NOT be called with `pml4_phys` equal to the kernel CR3 that has no
///   corresponding user address space — doing so would unmap the user windows
///   inside the kernel page tables, corrupting all future user tasks.
/// - Interrupts are disabled for the duration of the CR3 switch (handled internally
///   by [`with_address_space`]).
///
/// ## `release_user_code_pfns` policy
/// - `false`: clear user-code mappings but keep mapped code PFNs reserved
///   (safe for temporary user aliases of kernel text pages).
/// - `true`: release user-code PFNs back to PMM (required for loader-owned images).
pub fn destroy_user_address_space_with_options(pml4_phys: u64, release_user_code_pfns: bool) {
    // Default behavior: tear down full configured user code + stack windows.
    destroy_user_address_space_with_page_counts(
        pml4_phys,
        release_user_code_pfns,
        (USER_CODE_SIZE / PAGE_SIZE_U64) as usize,
        (USER_STACK_SIZE / PAGE_SIZE_U64) as usize,
    );
}

/// Destroys a user address space with explicit mapped-page counts.
///
/// This variant is intended for callers that know exactly how many pages were
/// mapped and can therefore avoid scanning full user regions.
///
/// `stack_page_count_from_top` is interpreted as a contiguous window growing
/// downward from [`USER_STACK_TOP`], matching how user stacks are allocated.
///
/// Count values are clamped to configured region capacities.
pub fn destroy_user_address_space_with_page_counts(
    pml4_phys: u64,
    release_user_code_pfns: bool,
    code_page_count: usize,
    stack_page_count_from_top: usize,
) {
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
        // Step 1: Drop user-code mappings for the known mapped prefix.
        // Caller controls whether mapped code PFNs are returned to PMM.
        let mut va = USER_CODE_BASE;
        for _ in 0..code_pages {
            unmap_page_and_prune_pagetable_hierarchy(va, release_user_code_pfns);
            va += PAGE_SIZE_U64;
        }

        // Step 2: Drop mapped user-stack pages in the top-down stack window.
        // Stack pages are always process-owned, so leaf PFNs are always released.
        let mut stack_va = USER_STACK_TOP - (stack_pages as u64 * PAGE_SIZE_U64);
        while stack_va < USER_STACK_TOP {
            unmap_page_and_prune_pagetable_hierarchy(stack_va, true);
            stack_va += PAGE_SIZE_U64;
        }

        // Step 3: Clear and release all mapped pages in the user-mode heap region.
        unmap_user_heap_region();
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
    let mut va = USER_HEAP_BASE;
    while va < USER_HEAP_END {
        // Step 1: Resolve PML4 level and skip if non-present.
        let pml4 = table_at(PML4_TABLE_ADDR);
        let pml4_idx = pml4_index(va);
        let pml4e = table_entry(pml4, pml4_idx);
        if !pml4e.present() {
            // PML4 entries cover 512 GiB of virtual address space.
            va = (va + 0x80_0000_0000) & !(0x80_0000_0000 - 1);
            continue;
        }

        // Step 2: Resolve PDPT level and skip if non-present.
        let pdp = table_at(pdp_table_addr(va));
        let pdp_idx = pdp_index(va);
        let pdpe = table_entry(pdp, pdp_idx);
        if !pdpe.present() {
            // PDPT entries cover 1 GiB of virtual address space.
            va = (va + 0x4000_0000) & !(0x4000_0000 - 1);
            continue;
        }

        // Step 3: Resolve PD level and skip if non-present.
        let pd = table_at(pd_table_addr(va));
        let pd_idx = pd_index(va);
        let pde = table_entry(pd, pd_idx);
        if !pde.present() {
            // PD entries cover 2 MiB of virtual address space.
            va = (va + 0x20_0000) & !(0x20_0000 - 1);
            continue;
        }

        // Step 4: Page table level exists; unmap individual page and advance by page size.
        unmap_page_and_prune_pagetable_hierarchy(va, true);
        va += PAGE_SIZE_U64;
    }
}

/// Maps one user virtual page to `pfn` using user-accessible permissions.
///
/// `virtual_address` must be within configured user code/stack regions and
/// must not target the configured guard page.
///
/// # Safety
/// This function mutates page tables via recursive mapping and therefore
/// requires a stable active address space while it runs.
///
/// Callers must execute it only inside `with_address_space` (or an equivalent
/// critical section) that:
/// - disables interrupts for the full duration, and
/// - guarantees `CR3` does not change until the function returns.
///
/// If this precondition is violated, a context switch can switch to a different
/// `CR3` while recursive addresses are being resolved, which can race and write
/// into the wrong page-table hierarchy.
pub fn map_user_page(virtual_address: u64, pfn: u64, writable: bool) -> Result<(), MapError> {
    // Normalize to 4 KiB page granularity; callers may pass any address
    // within the target page.
    let virtual_address = page_align_down(virtual_address);

    // Enforce user-window policy before touching page tables.
    // Derive the NX policy from the region:
    //   - CODE  → no_execute = false  (pages must be executable)
    //   - STACK → no_execute = true   (pages must not be executable; prevents stack injection)
    //   - HEAP  → no_execute = true
    // EFER.NXE is activated in kaosldr_16/longmode.asm; without it bit 63 is ignored by the CPU.
    let no_execute = match classify_user_region(virtual_address) {
        Some(UserRegion::Code) => false,
        Some(UserRegion::Stack) => true,
        Some(UserRegion::Heap) => true,
        Some(UserRegion::Guard) => {
            return Err(MapError::UserGuardPage { virtual_address });
        }
        None => {
            return Err(MapError::NotUserRegion { virtual_address });
        }
    };

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
            // Propagate NX policy: stack pages become non-executable, code pages stay executable.
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

        // Apply NX policy: stack pages are non-executable, code pages are executable.
        (*e).set_no_execute(no_execute);
    }

    // Invalidate stale translation for this VA in current TLB context.
    invlpg(virtual_address);

    Ok(())
}
