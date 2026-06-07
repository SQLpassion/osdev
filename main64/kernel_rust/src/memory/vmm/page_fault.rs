use crate::arch::constants::PAGE_SIZE_U64;

use super::page_table::{
    alloc_frame_phys, entry_ptr, invlpg, is_leaf_present, page_align_down, phys_to_pfn, pml4_index,
    pdp_index, pd_index, pt_index, read_cr3, table_at, table_entry, pt_table_addr, zero_virt_page,
};
use super::{
    classify_user_region, debug_alloc, debug_enabled, populate_page_table_path, vmm_logln,
    UserRegion, USER_STACK_TOP,
};

pub const PF_ERR_PRESENT: u64 = 1 << 0;

/// Error returned by the checked page-fault handling path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultError {
    /// Fault had `P=1` in the CPU error code, i.e. a protection violation.
    /// These faults must not trigger demand allocation.
    ProtectionFault {
        virtual_address: u64,
        error_code: u64,
    },

    /// Fault could not be handled because PMM ran out of physical frames.
    OutOfMemory {
        virtual_address: u64,
        error_code: u64,
    },
}

/// Handles page faults by demand-allocating page tables and target page frame.
///
/// Returns `Err(PageFaultError::ProtectionFault)` for protection faults (`P=1`),
/// `Err(PageFaultError::OutOfMemory)` when PMM allocation fails, and `Ok(())`
/// for handled non-present faults.
pub fn try_handle_page_fault(virtual_address: u64, error_code: u64) -> Result<(), PageFaultError> {
    // Keep both raw and page-aligned addresses for diagnostics and mapping.
    let fault_address_raw = virtual_address;
    let virtual_address = page_align_down(fault_address_raw);

    // Optional structured debug trace for fault triage.
    if debug_enabled() {
        let cr3 = read_cr3();

        vmm_logln(format_args!(
            "VMM: page fault raw=0x{:x} aligned=0x{:x} cr3=0x{:x} err=0x{:x}",
            fault_address_raw, virtual_address, cr3, error_code
        ));

        vmm_logln(format_args!(
            "VMM: indices pml4={} pdp={} pd={} pt={}",
            pml4_index(virtual_address),
            pdp_index(virtual_address),
            pd_index(virtual_address),
            pt_index(virtual_address)
        ));

        vmm_logln(format_args!(
            "VMM: err bits p={} w={} u={} rsv={} ifetch={}",
            (error_code & (1 << 0)) != 0,
            (error_code & (1 << 1)) != 0,
            (error_code & (1 << 2)) != 0,
            (error_code & (1 << 3)) != 0,
            (error_code & (1 << 4)) != 0
        ));
    }

    // Only demand-map on non-present faults.
    // Protection faults indicate a real access violation and must not be hidden.
    if (error_code & PF_ERR_PRESENT) != 0 {
        vmm_logln(format_args!(
            "VMM: protection fault at 0x{:x} err=0x{:x} (allocation refused)",
            fault_address_raw, error_code
        ));
        return Err(PageFaultError::ProtectionFault {
            virtual_address: fault_address_raw,
            error_code,
        });
    }

    let user_region = classify_user_region(virtual_address);
    // Guard page faults are always treated as protection violations.
    if matches!(user_region, Some(UserRegion::Guard)) {
        return Err(PageFaultError::ProtectionFault {
            virtual_address: fault_address_raw,
            error_code,
        });
    }

    let user_access = matches!(
        user_region,
        Some(UserRegion::Code) | Some(UserRegion::Stack)
    );

    // Derive final permissions from the region:
    //   USER_CODE  → read-only  (writable=false), executable  (no_execute=false)
    //   USER_STACK → writable   (writable=true),  non-executable (no_execute=true)
    //   kernel     → writable   (writable=true),  no NX applied (kernel code/data mixed)
    // EFER.NXE is activated in kaosldr_16/longmode.asm; without it no_execute is ignored.
    let writable = !matches!(user_region, Some(UserRegion::Code));
    let no_execute = matches!(user_region, Some(UserRegion::Stack));

    // Step 1: user-stack faults grow downward stack pages on demand.
    if matches!(user_region, Some(UserRegion::Stack)) {
        return demand_map_user_stack_growth(virtual_address, fault_address_raw, error_code);
    }

    // Step 2: non-stack faults use single-page demand mapping.
    demand_map_leaf_page(
        virtual_address,
        user_access,
        writable,
        no_execute,
        fault_address_raw,
        error_code,
    )
}

/// Ensures one virtual page is mapped according to demand-paging policy.
pub fn demand_map_leaf_page(
    virtual_address: u64,
    user_access: bool,
    writable: bool,
    no_execute: bool,
    fault_address_raw: u64,
    error_code: u64,
) -> Result<(), PageFaultError> {
    // Step 1: ensure page-table path exists for this VA; OOM is recoverable.
    if populate_page_table_path(virtual_address, user_access).is_err() {
        return Err(PageFaultError::OutOfMemory {
            virtual_address: fault_address_raw,
            error_code,
        });
    }

    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Step 2: allocate and zero only when the leaf is currently non-present.
    if !table_entry(pt, pt_idx).present() {
        let Some(new_page_phys) = alloc_frame_phys() else {
            return Err(PageFaultError::OutOfMemory {
                virtual_address: fault_address_raw,
                error_code,
            });
        };

        // Map writable first so zero-fill is valid even for final read-only pages.
        // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled in page-fault handler.
        unsafe {
            (*entry_ptr(pt, pt_idx)).set_mapping(
                phys_to_pfn(new_page_phys),
                true,
                true,
                user_access,
            )
        };
        invlpg(virtual_address);
        zero_virt_page(virtual_address);

        // Step 3: tighten final permissions and NX policy after zero-fill.
        if !writable {
            // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
            unsafe { (*entry_ptr(pt, pt_idx)).set_writable(false) };
        }

        if no_execute {
            // SAFETY: `pt` is a valid PT page, `pt_idx < PT_ENTRIES`, interrupts disabled.
            unsafe { (*entry_ptr(pt, pt_idx)).set_no_execute(true) };
        }

        if !writable || no_execute {
            invlpg(virtual_address);
        }

        debug_alloc("PT", pt_idx, table_entry(pt, pt_idx).frame());
    }

    Ok(())
}

/// Grows the user stack downward from `fault_page_va` up to the nearest mapped page above it.
///
/// This keeps stack growth contiguous in virtual space even when the fault lands
/// several pages below the current mapped top due to large stack adjustments.
pub fn demand_map_user_stack_growth(
    fault_page_va: u64,
    fault_address_raw: u64,
    error_code: u64,
) -> Result<(), PageFaultError> {
    // Step 1: find the first already-mapped stack page above the fault.
    // If none exists, grow at most up to `USER_STACK_TOP`.
    let mut upper_bound_exclusive = USER_STACK_TOP;
    let mut probe_va = fault_page_va.saturating_add(PAGE_SIZE_U64);
    while probe_va < USER_STACK_TOP {
        if is_leaf_present(probe_va) {
            upper_bound_exclusive = probe_va;
            break;
        }
        probe_va = probe_va.saturating_add(PAGE_SIZE_U64);
    }

    // Step 2: map the missing range [fault_page_va, upper_bound_exclusive) as
    // writable + NX user stack pages.
    let mut map_va = fault_page_va;
    while map_va < upper_bound_exclusive {
        demand_map_leaf_page(map_va, true, true, true, fault_address_raw, error_code)?;
        map_va = map_va.saturating_add(PAGE_SIZE_U64);
    }

    Ok(())
}

/// Handles page faults for production interrupt paths.
///
/// This wrapper preserves the existing behavior: protection faults are fatal.
pub fn handle_page_fault(virtual_address: u64, error_code: u64) {
    // Production path keeps historical behavior: unrecoverable faults are fatal.
    match try_handle_page_fault(virtual_address, error_code) {
        Ok(()) => {}
        Err(PageFaultError::ProtectionFault {
            virtual_address,
            error_code,
        }) => {
            panic!(
                "VMM: protection page fault at 0x{:x} err=0x{:x}",
                virtual_address, error_code
            );
        }
        Err(PageFaultError::OutOfMemory {
            virtual_address,
            error_code,
        }) => {
            panic!(
                "VMM: out of physical memory while handling page fault at 0x{:x} err=0x{:x}",
                virtual_address, error_code
            );
        }
    }
}
