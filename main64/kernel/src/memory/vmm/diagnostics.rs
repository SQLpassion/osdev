#![allow(dead_code)]

use super::page_table::{
    page_align_down, pml4_index, pdp_index, pd_index, pt_index, table_at, table_entry,
    pt_for_if_present, pdp_table_addr, pd_table_addr, pt_table_addr, PML4_TABLE_ADDR,
};
use super::{
    vmm_logln, write_virt_u8, read_virt_u8, unmap_virtual_address,
};

/// Returns page-table PFNs `(pdp, pd, pt)` for `virtual_address` in active CR3.
///
/// Intended for diagnostics and integration tests.
pub fn debug_table_pfns_for_va(virtual_address: u64) -> Option<(u64, u64, u64)> {
    // Diagnostics always inspect page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = table_entry(pml4, pml4_idx);

    if !pml4e.present() || pml4e.huge() {
        return None;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = table_entry(pdp, pdp_idx);

    if !pdpe.present() || pdpe.huge() {
        return None;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = table_entry(pd, pd_idx);

    if !pde.present() || pde.huge() {
        return None;
    }

    // Return intermediate table frame numbers for caller-side inspection.
    Some((pml4e.frame(), pdpe.frame(), pde.frame()))
}

/// Returns the mapped leaf PFN for `virtual_address` in active CR3.
///
/// Intended for diagnostics and integration tests.
pub fn debug_mapped_pfn_for_va(virtual_address: u64) -> Option<u64> {
    // Diagnostics always inspect page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pt = pt_for_if_present(virtual_address)?;
    let pte = table_entry(pt, pt_index(virtual_address));

    // Return leaf PFN only when mapping is currently present.
    if pte.present() {
        Some(pte.frame())
    } else {
        None
    }
}

/// Returns mapping user/writable flags `(pml4_u, pdp_u, pd_u, pt_u, pt_w)` for `virtual_address`.
///
/// Intended for diagnostics and integration tests.
pub fn debug_mapping_flags_for_va(virtual_address: u64) -> Option<(bool, bool, bool, bool, bool)> {
    // Diagnostics always inspect page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = table_entry(pml4, pml4_idx);

    if !pml4e.present() || pml4e.huge() {
        return None;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = table_entry(pdp, pdp_idx);

    if !pdpe.present() || pdpe.huge() {
        return None;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = table_entry(pd, pd_idx);

    if !pde.present() || pde.huge() {
        return None;
    }

    let pt = table_at(pt_table_addr(virtual_address));
    let pte = table_entry(pt, pt_index(virtual_address));

    if !pte.present() {
        return None;
    }

    // Return "user" propagation across all levels plus leaf writability.
    Some((
        pml4e.user(),
        pdpe.user(),
        pde.user(),
        pte.user(),
        pte.writable(),
    ))
}

/// Returns whether the No-Execute bit (bit 63) is set in the leaf PTE for
/// `virtual_address` in the active CR3.
///
/// - `Some(true)`  → leaf entry present and NX bit set (page non-executable).
/// - `Some(false)` → leaf entry present and NX bit clear (page executable).
/// - `None`        → any page-table level missing, huge-mapped, or leaf not present.
///
/// Intended for diagnostics and integration tests only.
pub fn debug_no_execute_flag_for_va(virtual_address: u64) -> Option<bool> {
    // Diagnostics always inspect the page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pt = pt_for_if_present(virtual_address)?;
    let pte = table_entry(pt, pt_index(virtual_address));

    // Return NX state only when leaf mapping is currently present.
    if pte.present() {
        Some(pte.no_execute())
    } else {
        None
    }
}

/// Basic VMM smoke test that triggers page faults and verifies readback.
pub fn test_vmm() -> bool {
    // Step 1: force demand-mapping by writing to three sparse addresses.
    vmm_logln(format_args!("VMM test: start"));
    const TEST_ADDR1: u64 = 0xFFFF_8009_4F62_D000;
    const TEST_ADDR2: u64 = 0xFFFF_8034_C232_C000;
    const TEST_ADDR3: u64 = 0xFFFF_807F_7200_7000;
    vmm_logln(format_args!("VMM test: write to 0x{:x}", TEST_ADDR1));
    write_virt_u8(TEST_ADDR1, b'A');

    vmm_logln(format_args!("VMM test: write to 0x{:x}", TEST_ADDR2));
    write_virt_u8(TEST_ADDR2, b'B');

    vmm_logln(format_args!("VMM test: write to 0x{:x}", TEST_ADDR3));
    write_virt_u8(TEST_ADDR3, b'C');

    // Step 2: read back and validate data integrity.
    vmm_logln(format_args!("VMM test: readback and verify"));
    let v1 = read_virt_u8(TEST_ADDR1);
    let v2 = read_virt_u8(TEST_ADDR2);
    let v3 = read_virt_u8(TEST_ADDR3);

    let ok = v1 == b'A' && v2 == b'B' && v3 == b'C';
    if ok {
        vmm_logln(format_args!("VMM test: readback OK (A, B, C)"));
    } else {
        vmm_logln(format_args!(
            "VMM test: readback FAILED got [{:#x}, {:#x}, {:#x}] expected [0x41, 0x42, 0x43]",
            v1, v2, v3
        ));
    }

    // Unmap test pages so the next `vmmtest` run triggers page faults again.
    unmap_virtual_address(TEST_ADDR1);
    unmap_virtual_address(TEST_ADDR2);
    unmap_virtual_address(TEST_ADDR3);
    vmm_logln(format_args!("VMM test: unmapped test pages"));
    vmm_logln(format_args!("VMM test: done (ok={})", ok));
    vmm_logln(format_args!(""));
    ok
}
