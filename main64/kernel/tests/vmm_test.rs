//! Virtual Memory Manager Integration Tests
//!
//! This test boots a dedicated kernel, initializes PMM/VMM/IDT,
//! and runs the same smoke path as the `vmmtest` shell command.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::vmm::page_table::{
    build_kernel_pml4_from_firmware, entry_ptr, pd_index, pd_table_addr, pdp_index, pdp_table_addr,
    phys_to_pfn, pml4_index, pt_index, walk_levels, PageTable, WalkResult, ENTRY_HUGE,
    PDP_TABLE_BASE, PD_TABLE_BASE, PML4_TABLE_ADDR, PT_ENTRIES, PT_TABLE_BASE, RECURSIVE_SLOT,
};
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the VMM integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: vmm smoke once.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "vmm smoke once".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_vmm_smoke_once() {
    assert!(vmm::test_vmm(), "vmm::test_vmm() should succeed");
}

/// Contract: vmm smoke twice.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "vmm smoke twice".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_vmm_smoke_twice() {
    assert!(vmm::test_vmm(), "first vmm::test_vmm() run should succeed");
    assert!(vmm::test_vmm(), "second vmm::test_vmm() run should succeed");
}

/// Contract: kernel non-present faults outside heap range fail demand allocation.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "kernel non-present faults outside heap range fail demand allocation".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_kernel_non_present_fault_outside_heap_fails() {
    const OUTSIDE_KERNEL_VA: u64 = 0xFFFF_8091_2345_6000;
    vmm::unmap_virtual_address(OUTSIDE_KERNEL_VA);

    let res = vmm::try_handle_page_fault(OUTSIDE_KERNEL_VA, 0);
    assert_eq!(
        res,
        Err(vmm::PageFaultError::ProtectionFault {
            virtual_address: OUTSIDE_KERNEL_VA,
            error_code: 0,
        }),
        "non-present fault outside kernel heap arena must be refused"
    );
}

/// Contract: non present fault allocates and maps page.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "non present fault allocates and maps page".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_non_present_fault_allocates_and_maps_page() {
    const TEST_VA: u64 = 0xFFFF_8000_0200_2000;
    vmm::unmap_virtual_address(TEST_VA);

    vmm::try_handle_page_fault(TEST_VA, 0)
        .expect("non-present fault should be handled by demand allocation");

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `TEST_VA` was mapped by the handled non-present fault above.
    // - Volatile access targets exactly one byte in the mapped page.
    unsafe {
        let ptr = TEST_VA as *mut u8;
        core::ptr::write_volatile(ptr, 0x5A);
        let val = core::ptr::read_volatile(ptr);
        assert!(
            val == 0x5A,
            "mapped page should be writable after non-present fault"
        );
    }

    vmm::unmap_virtual_address(TEST_VA);
}

/// Contract: user stack fault mappings keep user path bits set and a writable
/// leaf; a code-region fault is rejected rather than demand-mapped (the ELF
/// loader pre-maps every segment with its final permissions, so a fault
/// landing in the code window is a real bug, not a hole to backfill).
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "code faults are rejected, stack faults keep user path bits set with a writable leaf".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_user_fault_mapping_rejects_code_and_maps_stack_writable() {
    let code_va = vmm::USER_CODE_BASE + 0x0011_5000;
    let stack_va = vmm::USER_STACK_TOP - 4096;

    vmm::unmap_virtual_address(code_va);
    vmm::unmap_virtual_address(stack_va);

    // Simulate non-present user faults (`U=1`, `P=0` -> error code 0x4).
    let code_result = vmm::try_handle_page_fault(code_va, 0x4);
    assert!(
        matches!(
            code_result,
            Err(vmm::PageFaultError::InvalidUserAccess { .. })
        ),
        "non-present code-region fault must be rejected, not demand-mapped"
    );
    assert!(
        vmm::debug_mapping_flags_for_va(code_va).is_none(),
        "rejected code-region fault must not install any mapping"
    );

    vmm::try_handle_page_fault(stack_va, 0x4)
        .expect("user stack non-present fault should be demand-mapped");

    let stack_flags = vmm::debug_mapping_flags_for_va(stack_va)
        .expect("stack VA should have present mapping flags");
    assert!(
        stack_flags == (true, true, true, true, true),
        "stack VA must have user path bits set and writable leaf"
    );

    vmm::unmap_virtual_address(stack_va);
}

/// Contract: heap demand fault maps user writable non-executable page.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap demand fault maps user writable non-executable page".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_demand_fault_maps_user_writable_non_executable_page() {
    let heap_va = vmm::USER_HEAP_BASE + 0x0234_5000;
    vmm::unmap_virtual_address(heap_va);

    // Simulate a non-present user-mode heap fault (U=1, P=0 -> error code 0x4).
    vmm::try_handle_page_fault(heap_va, 0x4)
        .expect("user heap non-present fault should be demand-mapped");

    let heap_flags = vmm::debug_mapping_flags_for_va(heap_va)
        .expect("heap VA should have present mapping flags");
    assert!(
        heap_flags == (true, true, true, true, true),
        "heap VA must have user path bits set and writable leaf, got {:?}",
        heap_flags
    );

    let heap_nx = vmm::debug_no_execute_flag_for_va(heap_va)
        .expect("demand-mapped heap VA must have a present leaf PTE");
    assert!(
        heap_nx,
        "demand-mapped heap leaf PTE must have No-Execute bit set"
    );

    vmm::unmap_virtual_address(heap_va);
}

/// Contract: faulted page is zero initialized.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "faulted page is zero initialized".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_faulted_page_is_zero_initialized() {
    const TEST_VA: u64 = 0xFFFF_8000_0200_0000;
    vmm::unmap_virtual_address(TEST_VA);

    vmm::try_handle_page_fault(TEST_VA, 0)
        .expect("non-present fault should be handled by demand allocation");

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `TEST_VA` was mapped by demand paging above.
    // - Access stays within first and last byte of that mapped page.
    unsafe {
        let base = TEST_VA as *mut u8;
        core::ptr::write_volatile(base, 0xAB);
        core::ptr::write_volatile(base.add(4095), 0xCD);
    }

    vmm::unmap_virtual_address(TEST_VA);

    vmm::try_handle_page_fault(TEST_VA, 0)
        .expect("second non-present fault should demand-map new page");

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `TEST_VA` was remapped by demand paging above.
    // - Access is limited to the mapped page bounds.
    unsafe {
        let base = TEST_VA as *const u8;
        let first = core::ptr::read_volatile(base);
        let last = core::ptr::read_volatile(base.add(4095));
        assert!(first == 0, "first byte of faulted page should be zeroed");
        assert!(last == 0, "last byte of faulted page should be zeroed");
    }

    vmm::unmap_virtual_address(TEST_VA);
}

/// Contract: unmap absent address is noop.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "unmap absent address is noop".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_unmap_absent_address_is_noop() {
    const TEST_VA: u64 = 0xFFFF_8000_0200_3000;

    // Must not fault even if no paging path exists yet.
    vmm::unmap_virtual_address(TEST_VA);
    vmm::unmap_virtual_address(TEST_VA);

    // The address should still be demand-mappable afterwards.
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - First touch triggers demand mapping for this test VA.
    // - Subsequent volatile read/write stays within one byte.
    unsafe {
        let ptr = TEST_VA as *mut u8;
        core::ptr::write_volatile(ptr, 0x11);
        assert!(core::ptr::read_volatile(ptr) == 0x11);
    }

    vmm::unmap_virtual_address(TEST_VA);
}

/// Contract: protection fault returns error in checked path.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "protection fault returns error in checked path".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_protection_fault_returns_error_in_checked_path() {
    const TEST_VA: u64 = 0xFFFF_8094_5678_9000;
    let err = vmm::try_handle_page_fault(TEST_VA, 1)
        .expect_err("protection fault must not trigger allocation");
    assert!(
        matches!(
            err,
            vmm::PageFaultError::ProtectionFault {
                virtual_address: TEST_VA,
                error_code: 1
            }
        ),
        "expected PageFaultError::ProtectionFault with original fault data"
    );
}

/// Contract: try map rejects overwrite of existing mapping.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "try map rejects overwrite of existing mapping".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_try_map_rejects_overwrite_of_existing_mapping() {
    const TEST_VA: u64 = 0xFFFF_8095_6789_A000;
    vmm::unmap_virtual_address(TEST_VA);

    let frame_a = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame_a allocation failed"));
    let frame_b = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame_b allocation failed"));

    vmm::try_map_virtual_to_physical(TEST_VA, frame_a.physical_address())
        .expect("initial mapping should succeed");

    let err = vmm::try_map_virtual_to_physical(TEST_VA, frame_b.physical_address())
        .expect_err("overwriting existing mapping must be rejected");
    assert!(
        matches!(
            err,
            vmm::MapError::AlreadyMapped {
                virtual_address: TEST_VA,
                current_pfn: a,
                requested_pfn: b
            } if a == frame_a.pfn && b == frame_b.pfn
        ),
        "expected AlreadyMapped error with current/requested PFNs"
    );

    vmm::unmap_virtual_address(TEST_VA);
    // frame_a is released by unmap; frame_b was never mapped, release it here.
    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(frame_b.pfn)));
}

/// Contract: unmap releases frame back to pmm.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "unmap releases frame back to pmm".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_unmap_releases_frame_back_to_pmm() {
    const TEST_VA: u64 = 0xFFFF_8096_789A_B000;
    vmm::unmap_virtual_address(TEST_VA);

    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame allocation failed"));
    let mapped_pfn = frame.pfn;
    vmm::try_map_virtual_to_physical(TEST_VA, frame.physical_address())
        .expect("mapping should succeed");

    vmm::unmap_virtual_address(TEST_VA);

    let reused = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("re-allocation failed"));
    assert!(
        reused.pfn == mapped_pfn,
        "unmap should release mapped frame back to PMM for reuse"
    );
    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(reused.pfn)));
}

/// Contract: clone kernel pml4 for user returns distinct pml4 with self recursive entry.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "clone kernel pml4 for user returns distinct pml4 with self recursive entry".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_clone_kernel_pml4_for_user_returns_distinct_pml4_with_self_recursive_entry() {
    const TEMP_CLONE_VIEW: u64 = 0xFFFF_8097_1111_0000;
    const ENTRY_FRAME_MASK: u64 = 0x0000_FFFF_FFFF_F000;

    vmm::unmap_virtual_address(TEMP_CLONE_VIEW);
    let kernel_pml4 = vmm::get_pml4_address();
    let clone_pml4 = vmm::clone_kernel_pml4_for_user();
    assert!(
        clone_pml4 != kernel_pml4,
        "clone must allocate a distinct PML4 frame"
    );

    vmm::map_virtual_to_physical(TEMP_CLONE_VIEW, clone_pml4);
    let recursive_entry = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it performs volatile access through a raw pointer.
        // - `TEMP_CLONE_VIEW` is mapped to the clone PML4 page.
        // - Entry index 511 is in-bounds for one 4KiB table page.
        core::ptr::read_volatile((TEMP_CLONE_VIEW as *const u64).add(511))
    };
    let recursive_phys = recursive_entry & ENTRY_FRAME_MASK;
    assert!(
        recursive_phys == clone_pml4,
        "clone PML4 entry 511 must self-reference clone frame"
    );

    // Releases clone frame via standard unmap path.
    vmm::unmap_virtual_address(TEMP_CLONE_VIEW);
}

/// Contract: with address space switches cr3 for closure and restores previous cr3.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "with address space switches cr3 for closure and restores previous cr3".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_with_address_space_switches_cr3_for_closure_and_restores_previous_cr3() {
    let kernel_cr3 = vmm::get_pml4_address();
    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    assert!(user_cr3 != 0, "cloned user CR3 must be non-zero");

    let token = vmm::with_address_space(user_cr3, || {
        assert!(
            vmm::get_active_cr3() == user_cr3,
            "closure must observe target CR3 as active address space"
        );
        0xC0DEu64
    });
    assert!(
        token == 0xC0DEu64,
        "closure return value must be propagated"
    );

    assert!(
        vmm::get_active_cr3() == kernel_cr3,
        "with_address_space must restore the previous CR3 after closure returns"
    );

    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(user_cr3 / pmm::PAGE_SIZE)));
}

/// Contract: kernel pml4 accessor stays stable across temporary address-space switches.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "kernel pml4 accessor stays stable across temporary address-space switches".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_kernel_pml4_accessor_remains_kernel_root_across_with_address_space() {
    let kernel_cr3 = vmm::get_pml4_address();
    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    assert!(user_cr3 != 0, "cloned user CR3 must be non-zero");
    assert!(
        vmm::get_active_cr3() == kernel_cr3,
        "active CR3 should start at kernel root in this test context"
    );

    vmm::with_address_space(user_cr3, || {
        assert!(
            vmm::get_active_cr3() == user_cr3,
            "temporary switch must activate the requested user CR3"
        );
        assert!(
            vmm::get_pml4_address() == kernel_cr3,
            "kernel root accessor must stay stable during temporary user switch"
        );
    });

    assert!(
        vmm::get_active_cr3() == kernel_cr3,
        "active CR3 must restore to kernel root after temporary switch"
    );
    assert!(
        vmm::get_pml4_address() == kernel_cr3,
        "kernel root accessor must still return the canonical kernel CR3"
    );

    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(user_cr3 / pmm::PAGE_SIZE)));
}

/// Contract: destroy user address space releases user leaf and table frames.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space releases user leaf and table frames".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_releases_user_leaf_and_table_frames() {
    const TEST_USER_VA: u64 = vmm::USER_STACK_TOP - 4096;

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let leaf_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("leaf frame allocation failed"));

    let (pdp_pfn, pd_pfn, pt_pfn, leaf_pfn) = vmm::with_address_space(user_cr3, || {
        vmm::map_user_page(TEST_USER_VA, leaf_frame.pfn, true)
            .expect("test user VA should map in cloned address space");

        let (pdp, pd, pt) = vmm::debug_table_pfns_for_va(TEST_USER_VA)
            .expect("mapped user VA must have page-table chain");
        let mapped_leaf = vmm::debug_mapped_pfn_for_va(TEST_USER_VA)
            .expect("mapped user VA must have a present leaf PTE");
        (pdp, pd, pt, mapped_leaf)
    });

    assert!(
        leaf_pfn == leaf_frame.pfn,
        "mapped leaf PFN must match allocated data frame"
    );

    vmm::destroy_user_address_space(user_cr3);

    pmm::with_pmm(|mgr| {
        assert!(
            !mgr.release_pfn(leaf_pfn),
            "leaf data frame should already be free after address-space destroy"
        );
        assert!(
            !mgr.release_pfn(pt_pfn),
            "PT frame should already be free after address-space destroy"
        );
        assert!(
            !mgr.release_pfn(pd_pfn),
            "PD frame should already be free after address-space destroy"
        );
        assert!(
            !mgr.release_pfn(pdp_pfn),
            "PDP frame should already be free after address-space destroy"
        );
        assert!(
            !mgr.release_pfn(user_cr3 / pmm::PAGE_SIZE),
            "PML4 root frame should already be free after address-space destroy"
        );
    });
}

/// Contract: destroy user address space releases a single-owner code leaf frame.
///
/// Refcounting replaced the old `release_user_code_pfns` boolean policy: a
/// code frame with exactly one owner (the common case — a loader-owned
/// binary's private code frame) is now always released by
/// `destroy_user_address_space`, regardless of which region it is mapped in.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space releases a single-owner code leaf frame".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_releases_single_owner_code_leaf_frame() {
    const TEST_CODE_VA: u64 = vmm::USER_CODE_BASE;

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let code_leaf = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("code frame allocation failed"));

    vmm::with_address_space(user_cr3, || {
        vmm::map_user_page(TEST_CODE_VA, code_leaf.pfn, false)
            .expect("test code VA should map in cloned address space");
    });

    vmm::destroy_user_address_space(user_cr3);

    pmm::with_pmm(|mgr| {
        assert!(
            !mgr.release_pfn(code_leaf.pfn),
            "single-owner code-leaf PFN should already be free after address-space destroy"
        );
    });
}

/// Contract: destroy user address space keeps a shared (refcounted) code leaf frame
/// allocated until its other owner also releases it.
///
/// This is the core new behavior this refactor introduces: instead of a
/// caller-chosen boolean policy, a frame that is deliberately aliased into a
/// second mapping (via `inc_refcount`, mirroring e.g. a user-code window
/// pointing at a frame another mapping still references) survives address
/// -space teardown and is only actually freed once every owner has released
/// it.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space keeps a shared code leaf frame allocated until its other owner also releases it".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_keeps_shared_code_leaf_frame_until_last_release() {
    const TEST_CODE_VA: u64 = vmm::USER_CODE_BASE;
    const OUTER_ALIAS_VA: u64 = 0xFFFF_8098_789A_B000;

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let code_leaf = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("code frame allocation failed"));

    // Create a second, independent owner of the same physical frame: map it
    // into the currently active (kernel) address space at a scratch VA, and
    // record the extra ownership in PMM via `inc_refcount`.
    vmm::unmap_virtual_address(OUTER_ALIAS_VA);
    vmm::map_virtual_to_physical(OUTER_ALIAS_VA, code_leaf.physical_address());
    assert!(
        pmm::with_pmm(|mgr| mgr.inc_refcount(code_leaf.pfn)),
        "inc_refcount must succeed on an allocated frame"
    );

    vmm::with_address_space(user_cr3, || {
        vmm::map_user_page(TEST_CODE_VA, code_leaf.pfn, false)
            .expect("test code VA should map in cloned address space");
    });

    // Destroying the user address space releases only the user-side mapping's
    // ownership; the outer alias still owns the frame, so it must survive.
    vmm::destroy_user_address_space(user_cr3);

    pmm::with_pmm(|mgr| {
        assert!(
            mgr.release_pfn(code_leaf.pfn),
            "shared code-leaf PFN must still be allocated (and thus releasable) \
             immediately after destroy, because the outer alias still owns it"
        );
    });

    // The explicit `release_pfn` above just consumed the outer alias's
    // ownership (the last one), so the frame is now actually free.
    // Clean up the outer alias's page-table mapping too (best effort; the PFN
    // itself is already released).
    vmm::unmap_without_release(OUTER_ALIAS_VA);
}

/// Contract: destroy user address space with page counts releases mapped code/stack pages.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space with page counts releases mapped code/stack pages".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_with_page_counts_releases_mapped_code_and_stack_leaf_frames() {
    let code_va = vmm::USER_CODE_BASE;
    let stack_va = vmm::USER_STACK_TOP - 4096;

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let code_leaf = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("code frame allocation failed"));
    let stack_leaf = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("stack frame allocation failed"));

    vmm::with_address_space(user_cr3, || {
        vmm::map_user_page(code_va, code_leaf.pfn, false)
            .expect("code page should map in cloned address space");
        vmm::map_user_page(stack_va, stack_leaf.pfn, true)
            .expect("stack page should map in cloned address space");
    });

    // Exactly one mapped code page at USER_CODE_BASE and one mapped stack page
    // at USER_STACK_TOP-4KiB should be torn down.
    vmm::destroy_user_address_space_with_page_counts(user_cr3, 1, 1);

    pmm::with_pmm(|mgr| {
        assert!(
            !mgr.release_pfn(code_leaf.pfn),
            "count-based destroy must release mapped code leaf PFN"
        );
        assert!(
            !mgr.release_pfn(stack_leaf.pfn),
            "count-based destroy must release mapped stack leaf PFN"
        );
        assert!(
            !mgr.release_pfn(user_cr3 / pmm::PAGE_SIZE),
            "count-based destroy must release user CR3 root PFN"
        );
    });
}

/// Contract: destroy user address space reclaims mappings outside the fixed
/// Code/Stack/Heap windows (e.g. a future `mmap`-created region).
///
/// This exercises the generic catch-all reclaim pass `destroy_user_address_space`
/// runs after tearing down the three named windows: any VA still present in
/// the user PML4 slot range gets unmapped and its leaf frame released too, so
/// a region outside those three windows cannot be silently leaked at teardown.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space reclaims mappings outside the fixed Code/Stack/Heap windows".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_reclaims_mapping_outside_known_regions() {
    // Just above the heap window, well below the stack guard page: not Code,
    // not Stack/Guard, not Heap — i.e. classify_user_region returns None.
    const OTHER_REGION_VA: u64 = vmm::USER_HEAP_END + 0x0010_0000;
    assert!(
        vmm::classify_user_region(OTHER_REGION_VA).is_none(),
        "test VA must not fall inside any of the three known user regions"
    );

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let leaf = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("leaf frame allocation failed"));

    let mapped_pfn = vmm::with_address_space(user_cr3, || {
        vmm::try_map_virtual_to_physical(OTHER_REGION_VA, leaf.physical_address())
            .expect("mapping an out-of-window VA should still succeed at the page-table level");
        vmm::debug_mapped_pfn_for_va(OTHER_REGION_VA).expect("mapped leaf must resolve")
    });
    assert_eq!(
        mapped_pfn, leaf.pfn,
        "mapped leaf PFN must match the allocated frame"
    );

    vmm::destroy_user_address_space(user_cr3);

    pmm::with_pmm(|mgr| {
        assert!(
            !mgr.release_pfn(leaf.pfn),
            "teardown's catch-all pass must have already reclaimed the out-of-window mapping"
        );
    });
}

/// Contract: map user page accepts code and stack regions.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "map user page accepts code and stack regions".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_map_user_page_accepts_code_and_stack_regions() {
    let code_va = vmm::USER_CODE_BASE;
    let stack_va = vmm::USER_STACK_TOP - 4096;

    vmm::unmap_virtual_address(code_va);
    vmm::unmap_virtual_address(stack_va);

    let code_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("code frame alloc failed"));
    let stack_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("stack frame alloc failed"));

    vmm::map_user_page(code_va, code_frame.pfn, true)
        .expect("code page in user region should be mappable");
    vmm::map_user_page(stack_va, stack_frame.pfn, true)
        .expect("stack page in user region should be mappable");

    unsafe {
        // SAFETY:
        // - This requires `unsafe` because it reads/writes memory via raw virtual-address pointers.
        // - Both pages were mapped writable just above.
        core::ptr::write_volatile(code_va as *mut u8, 0xA5);
        core::ptr::write_volatile(stack_va as *mut u8, 0x5A);
        assert!(core::ptr::read_volatile(code_va as *const u8) == 0xA5);
        assert!(core::ptr::read_volatile(stack_va as *const u8) == 0x5A);
    }

    vmm::unmap_virtual_address(code_va);
    vmm::unmap_virtual_address(stack_va);
}

/// Contract: map user page rejects guard and non user regions.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "map user page rejects guard and non user regions".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_map_user_page_rejects_guard_and_non_user_regions() {
    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame alloc failed"));

    let guard_err = vmm::map_user_page(vmm::USER_STACK_GUARD_BASE, frame.pfn, true)
        .expect_err("guard page mapping must be rejected");
    assert!(
        matches!(guard_err, vmm::MapError::UserGuardPage { .. }),
        "guard-page mapping must return UserGuardPage error"
    );

    let outside_va = 0xFFFF_8000_0010_0000u64;
    let outside_err = vmm::map_user_page(outside_va, frame.pfn, true)
        .expect_err("non-user region mapping must be rejected");
    assert!(
        matches!(outside_err, vmm::MapError::NotUserRegion { virtual_address } if virtual_address == outside_va),
        "non-user address must return NotUserRegion with original address"
    );

    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(frame.pfn)));
}

/// Contract: map user page sets no execute bit on stack page.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "map user page sets no execute bit on stack page".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_map_user_page_sets_no_execute_bit_on_stack_page() {
    // Stack page one slot below the top of the user stack region.
    let stack_va = vmm::USER_STACK_TOP - 4096;
    vmm::unmap_virtual_address(stack_va);

    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("stack frame alloc failed"));
    vmm::map_user_page(stack_va, frame.pfn, true).expect("stack page should map successfully");

    // Stack pages must be non-executable to prevent code injection via stack overflows.
    // EFER.NXE is activated in kaosldr_16/longmode.asm; bit 63 in the PTE is only
    // effective after that MSR write.
    let nx = vmm::debug_no_execute_flag_for_va(stack_va)
        .expect("mapped stack VA must have a present leaf PTE");
    assert!(nx, "stack leaf PTE must have No-Execute bit set");

    vmm::unmap_virtual_address(stack_va);
}

/// Contract: map user page clears no execute bit on code page.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "map user page clears no execute bit on code page".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_map_user_page_clears_no_execute_bit_on_code_page() {
    // Code page at the start of the user executable window.
    let code_va = vmm::USER_CODE_BASE;
    vmm::unmap_virtual_address(code_va);

    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("code frame alloc failed"));

    // Step 1: initial writable mapping (mirrors what the loader does while copying bytes).
    vmm::map_user_page(code_va, frame.pfn, true)
        .expect("code page writable mapping should succeed");

    let nx_writable = vmm::debug_no_execute_flag_for_va(code_va)
        .expect("mapped code VA must have a present leaf PTE after writable map");
    assert!(
        !nx_writable,
        "code leaf PTE must not have No-Execute bit after writable map"
    );

    // Step 2: permission-update path — same PFN, read-only (mirrors the loader's second pass).
    vmm::map_user_page(code_va, frame.pfn, false)
        .expect("code page permission downgrade to read-only should succeed");

    let nx_readonly = vmm::debug_no_execute_flag_for_va(code_va)
        .expect("mapped code VA must have a present leaf PTE after read-only remap");
    assert!(
        !nx_readonly,
        "code leaf PTE must not have No-Execute bit after read-only remap"
    );

    vmm::unmap_virtual_address(code_va);
}

/// Contract: fault mapped stack page has no execute bit set.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "fault mapped stack page has no execute bit set".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_fault_mapped_stack_page_has_no_execute_bit_set() {
    // Stack page demand-mapped via the page-fault handler path.
    let stack_va = vmm::USER_STACK_TOP - 8192;
    vmm::unmap_virtual_address(stack_va);

    // Simulate a non-present user-mode stack fault (U=1, P=0 → error_code = 0x4).
    vmm::try_handle_page_fault(stack_va, 0x4)
        .expect("user stack non-present fault should be demand-mapped");

    // The demand-paging path must apply NX to stack pages to prevent injection attacks.
    let nx = vmm::debug_no_execute_flag_for_va(stack_va)
        .expect("demand-mapped stack VA must have a present leaf PTE");
    assert!(
        nx,
        "demand-mapped stack leaf PTE must have No-Execute bit set"
    );

    vmm::unmap_virtual_address(stack_va);
}

/// Contract: user stack fault grows contiguous pages up to mapped stack top.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "user stack fault grows contiguous pages up to mapped stack top".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_user_stack_fault_grows_contiguous_pages_up_to_mapped_top() {
    let top_page_va = vmm::USER_STACK_TOP - 4096;
    let mid_page_va = vmm::USER_STACK_TOP - 8192;
    let deep_page_va = vmm::USER_STACK_TOP - 12288;

    // Step 1: Prepare deterministic stack layout: only top bootstrap page mapped.
    vmm::unmap_virtual_address(deep_page_va);
    vmm::unmap_virtual_address(mid_page_va);
    vmm::unmap_virtual_address(top_page_va);

    let top_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("top stack frame alloc failed"));
    vmm::map_user_page(top_page_va, top_frame.pfn, true)
        .expect("top bootstrap stack page should map successfully");

    // Step 2: Faulting three pages below top must backfill missing intermediate pages.
    vmm::try_handle_page_fault(deep_page_va, 0x4)
        .expect("deep user stack non-present fault should trigger stack growth");

    // Step 3: Verify contiguous stack growth pages are now mapped as user+writable+NX.
    let deep_flags = vmm::debug_mapping_flags_for_va(deep_page_va)
        .expect("deep stack page should be mapped after demand growth");
    let mid_flags = vmm::debug_mapping_flags_for_va(mid_page_va)
        .expect("intermediate stack page should be mapped after demand growth");
    let top_flags = vmm::debug_mapping_flags_for_va(top_page_va)
        .expect("top stack page should remain mapped after demand growth");
    assert!(
        deep_flags == (true, true, true, true, true),
        "deep stack page must have user path bits set and writable leaf"
    );
    assert!(
        mid_flags == (true, true, true, true, true),
        "intermediate stack page must have user path bits set and writable leaf"
    );
    assert!(
        top_flags == (true, true, true, true, true),
        "top stack page must keep user path bits set and writable leaf"
    );

    let deep_nx = vmm::debug_no_execute_flag_for_va(deep_page_va)
        .expect("deep stack page must have a present leaf PTE");
    let mid_nx = vmm::debug_no_execute_flag_for_va(mid_page_va)
        .expect("intermediate stack page must have a present leaf PTE");
    assert!(deep_nx, "deep stack page must be non-executable");
    assert!(mid_nx, "intermediate stack page must be non-executable");

    vmm::unmap_virtual_address(deep_page_va);
    vmm::unmap_virtual_address(mid_page_va);
    vmm::unmap_virtual_address(top_page_va);
}

/// Contract: a non-present code-region fault is rejected at any offset inside
/// the code window, not just the one exercised by
/// `test_user_fault_mapping_rejects_code_and_maps_stack_writable`. The ELF
/// loader pre-maps every `PT_LOAD` segment with its final permissions before
/// the task runs, so there is no legitimate hole left anywhere in the code
/// window for the page-fault handler to backfill.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "a non-present code-region fault is rejected, not demand-mapped, at any offset".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_code_region_fault_is_rejected_at_any_offset() {
    let code_va = vmm::USER_CODE_BASE + 0x1000;
    vmm::unmap_virtual_address(code_va);

    // Simulate a non-present user-mode code fault (U=1, P=0 → error_code = 0x4).
    let result = vmm::try_handle_page_fault(code_va, 0x4);
    assert!(
        matches!(result, Err(vmm::PageFaultError::InvalidUserAccess { .. })),
        "non-present code-region fault must be rejected regardless of offset"
    );
    assert!(
        vmm::debug_mapping_flags_for_va(code_va).is_none(),
        "rejected code-region fault must not install any mapping"
    );

    vmm::unmap_virtual_address(code_va);
}

/// Allocates PMM frames until exhaustion and stores acquired PFNs into `held_pfns`.
///
/// Returns number of held PFNs. Panics if `held_pfns` is too small to observe OOM.
fn exhaust_pmm_frames(held_pfns: &mut [u64]) -> usize {
    let mut held_count = 0usize;

    pmm::with_pmm(|mgr| {
        // Step 1: Drain PMM by repeatedly allocating frames until `alloc_frame` returns None.
        while held_count < held_pfns.len() {
            let Some(frame) = mgr.alloc_frame() else {
                return;
            };
            held_pfns[held_count] = frame.pfn;
            held_count += 1;
        }

        // Step 2: If the buffer filled up before OOM, fail loudly to keep the test deterministic.
        if mgr.alloc_frame().is_some() {
            panic!("OOM test buffer too small; increase held_pfns capacity");
        }
    });

    held_count
}

/// Releases a PFN slice previously returned by `exhaust_pmm_frames`.
fn release_held_pfns(held_pfns: &[u64]) {
    pmm::with_pmm(|mgr| {
        // Restore all held frames so following tests (or cleanup paths) stay unaffected.
        for &pfn in held_pfns {
            assert!(
                mgr.release_pfn(pfn),
                "failed to release held PFN 0x{:x}",
                pfn
            );
        }
    });
}

/// Contract: map user page propagates out of memory from page table path setup.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "map user page propagates out of memory from page table path setup".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_aaa_map_user_page_propagates_out_of_memory_from_path_setup() {
    const MAX_HELD_PFNS: usize = 131_072;

    // Use a fresh user address space to ensure USER_CODE path tables are not pre-created.
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    vmm::with_address_space(user_cr3, || {
        let target_va = vmm::USER_CODE_BASE;
        vmm::unmap_virtual_address(target_va);

        // Step 1: Exhaust PMM so intermediate table allocation inside map_user_page must fail.
        let mut held_pfns = [0u64; MAX_HELD_PFNS];
        let held_count = exhaust_pmm_frames(&mut held_pfns);

        // Step 2: Mapping must return OutOfMemory (no panic, no partial rollback breakage).
        let err = vmm::map_user_page(target_va, 0x1234, true)
            .expect_err("map_user_page should propagate OOM from page-table path allocation");
        assert!(
            matches!(err, vmm::MapError::OutOfMemory { virtual_address } if virtual_address == target_va),
            "expected MapError::OutOfMemory for code VA path setup"
        );

        // Step 3: Restore PMM state inside this temporary address space.
        release_held_pfns(&held_pfns[..held_count]);
    });

    // Release the cloned PML4 root frame.
    vmm::destroy_user_address_space(user_cr3);
}

#[test_case]
fn test_vmm_contracts_doc_fix_issue_13() {
    // vmm is already imported
    // Just a sanity check that VMM is accessible and serial_debug is reachable,
    // which aligns with the scalar fields lock contract.
    let old = vmm::set_debug_output(true);
    let current = vmm::serial_debug_enabled();
    assert!(current);
    vmm::set_debug_output(old);
}

// ============================================================================
// Page-table logic tests (pure; no CR3 switch, no firmware, no QEMU devices).
//
// Covers the core of the UEFI `vmm::init` fix — `build_kernel_pml4_from_firmware` — plus
// the virtual-address index math the recursive mapping and higher half rely on. See
// `docs/vmm.md` §4. These touch only in-memory `PageTable` values, so they are fast and
// deterministic and do not depend on the PMM/VMM/heap state initialized above.
// ============================================================================

/// Contract: the kernel PML4 is a verbatim SUPERSET of the firmware PML4, plus a recursive
/// self-map at slot 511.
/// Given: a "firmware" PML4 filled with distinct entries in every slot.
/// When: build_kernel_pml4_from_firmware copies it into a fresh table for frame `dst_phys`.
/// Then: slots 0..=510 are byte-identical to the firmware table, and slot 511 is the
///       recursive self-map (present, writable, supervisor) pointing at `dst_phys`.
/// Failure Impact: regressing this re-introduces the minimal-map bug that reset real AMD
///       hardware at the CR3 switch (docs/vmm.md §4). Release-blocking.
#[test_case]
fn test_clone_copies_all_entries_and_sets_recursive() {
    let mut src = PageTable::new();
    let mut dst = PageTable::new();

    // Fill every firmware slot with a distinct, recognizable mapping.
    for i in 0..PT_ENTRIES {
        let pfn = (i as u64) + 0x10; // arbitrary non-zero, distinct per slot
        let present = true;
        let writable = i % 2 == 0;
        let user = i % 3 == 0;
        src.entries[i].set_mapping(pfn, present, writable, user);
    }

    // Arbitrary 4 KiB-aligned "physical" frame backing the destination table.
    let dst_phys: u64 = 0x0000_0007_FACE_0000;

    // SAFETY: both tables are valid, live stack objects; dst_phys stands in for dst's frame.
    unsafe {
        build_kernel_pml4_from_firmware(
            &src as *const PageTable,
            &mut dst as *mut PageTable,
            dst_phys,
        );
    }

    // Slots 0..=510 must be copied verbatim (raw bits identical).
    for i in 0..RECURSIVE_SLOT {
        assert_eq!(
            dst.entries[i].raw(),
            src.entries[i].raw(),
            "slot {i} must be a verbatim copy of the firmware entry"
        );
    }

    // Slot 511 must be the recursive self-map, NOT the copied firmware entry.
    let rec = dst.entries[RECURSIVE_SLOT];
    assert!(rec.present(), "recursive slot must be present");
    assert!(rec.writable(), "recursive slot must be writable");
    assert!(!rec.user(), "recursive slot must be supervisor-only");
    assert_eq!(
        rec.frame(),
        phys_to_pfn(dst_phys),
        "recursive slot must point at the PML4's own frame"
    );
    assert_ne!(
        rec.raw(),
        src.entries[RECURSIVE_SLOT].raw(),
        "recursive slot must override the firmware entry, not copy it"
    );
}

/// Contract: cloning does not mutate the source (firmware) table.
#[test_case]
fn test_clone_leaves_source_untouched() {
    let mut src = PageTable::new();
    for i in 0..PT_ENTRIES {
        src.entries[i].set_mapping((i as u64) + 1, true, true, false);
    }
    // Snapshot a few representative slots.
    let s0 = src.entries[0].raw();
    let s256 = src.entries[256].raw();
    let s511 = src.entries[511].raw();

    let mut dst = PageTable::new();
    // SAFETY: valid live tables.
    unsafe {
        build_kernel_pml4_from_firmware(
            &src as *const PageTable,
            &mut dst as *mut PageTable,
            0x1000,
        );
    }

    assert_eq!(src.entries[0].raw(), s0);
    assert_eq!(src.entries[256].raw(), s256);
    assert_eq!(src.entries[511].raw(), s511);
}

/// Contract: the higher-half base and kernel entry resolve to PML4 slot 256.
/// This is the slot the UEFI loader mirrors (PML4[0] -> PML4[256]) so the kernel can run
/// at 0xFFFF800000100000. Failure Impact: the higher half would map to the wrong slot.
#[test_case]
fn test_higher_half_indices() {
    assert_eq!(pml4_index(0xFFFF_8000_0000_0000), 256, "higher-half base");
    assert_eq!(pml4_index(0xFFFF_8000_0010_0000), 256, "kernel entry VA");
    // 0x100000 >> 12 == 0x100; & 0x1ff == 0x100.
    assert_eq!(
        pt_index(0xFFFF_8000_0010_0000),
        0x100,
        "kernel entry PT index"
    );
}

/// Contract: the recursive slot constant is 511, and the recursive self-VA decomposes into
/// all-511 indices at every level (the property that makes the recursive window work).
#[test_case]
fn test_recursive_indices() {
    assert_eq!(RECURSIVE_SLOT, 511);
    assert_eq!(pml4_index(0xFFFF_FFFF_FFFF_F000), 511);

    // PML4_TABLE_ADDR is the VA at which the PML4 maps itself: indices must be 511/511/511/511.
    assert_eq!(pml4_index(PML4_TABLE_ADDR), 511);
    assert_eq!(pdp_index(PML4_TABLE_ADDR), 511);
    assert_eq!(pd_index(PML4_TABLE_ADDR), 511);
    assert_eq!(pt_index(PML4_TABLE_ADDR), 511);
}

/// Contract: phys_to_pfn is a plain 4 KiB right-shift (PFN == addr / 4096).
#[test_case]
fn test_phys_to_pfn() {
    assert_eq!(phys_to_pfn(0x100000), 0x100);
    assert_eq!(phys_to_pfn(0), 0);
    assert_eq!(
        phys_to_pfn(0x0000_0007_FACE_0000),
        0x0000_0007_FACE_0000 >> 12
    );
}

/// Contract: the recursive-window base constants are the canonical sign-extended addresses.
/// Failure Impact: the VMM's recursive table windows would point at the wrong VAs.
#[test_case]
fn test_recursive_window_constants() {
    assert_eq!(PML4_TABLE_ADDR, 0xFFFF_FFFF_FFFF_F000);
    assert_eq!(PDP_TABLE_BASE, 0xFFFF_FFFF_FFE0_0000);
    assert_eq!(PD_TABLE_BASE, 0xFFFF_FFFF_C000_0000);
    assert_eq!(PT_TABLE_BASE, 0xFFFF_FF80_0000_0000);
}

// ---------------------------------------------------------------------------
// Issue #58: shared `walk_levels` page-table walk.
//
// The tests below exercise `page_table::walk_levels` directly against the
// live kernel CR3, covering every bail point (PML4 missing, PDP missing/huge,
// PD missing/huge, fully resolved) that used to be hand-duplicated across
// `pt_for_if_present`, `is_user_page_writable`, `is_user_page_readable`,
// `is_va_mapped`, the `diagnostics` helpers, and `mapping.rs`.
//
// All scratch virtual addresses below live in PML4 slot 257
// (`0xFFFF_8080_0000_0000`..`0xFFFF_8100_0000_0000`, see `TEMP_CLONE_PML4_VA`'s
// doc comment), the slot this test suite already uses for one-off scratch
// mappings, at PDP indices not touched by any other test in this file.
// ---------------------------------------------------------------------------

/// Contract: walk_levels reports Pml4Missing for a virtual address whose PML4
/// slot has never been populated.
/// Given: PML4 slot 292 (`0xFFFF_9200_0000_0000`) is not used by any boot path,
///        test fixture, or other test in this file.
/// When: walk_levels is called for an address in that slot.
/// Then: it returns WalkResult::Pml4Missing.
/// Failure Impact: the shared walk's top-level bail condition regressed; every
///       caller relying on it (is_va_mapped, is_user_page_writable/readable,
///       pt_for_if_present, the diagnostics helpers) would misbehave.
#[test_case]
fn test_walk_levels_reports_pml4_missing() {
    const UNUSED_SLOT_VA: u64 = 0xFFFF_9200_0000_0000;
    assert_eq!(pml4_index(UNUSED_SLOT_VA), 292, "sanity: expected slot 292");

    assert!(
        matches!(walk_levels(UNUSED_SLOT_VA), WalkResult::Pml4Missing),
        "an address in a never-populated PML4 slot must report Pml4Missing"
    );
}

/// Contract: walk_levels reports PdpMissing when PML4 is present but the PDP
/// entry was never created.
/// Given: an anchor page is mapped at PDP index 200 within slot 257 (forcing
///        PML4[257] to be present), and PDP index 205 within the same slot is
///        left untouched.
/// When: walk_levels is called for the untouched PDP index.
/// Then: it returns WalkResult::PdpMissing, carrying the resolved PML4 entry.
/// Failure Impact: same as above, one level deeper.
#[test_case]
fn test_walk_levels_reports_pdp_missing() {
    const ANCHOR_VA: u64 = 0xFFFF_8080_0000_0000 + (200u64 << 30);
    const TARGET_VA: u64 = 0xFFFF_8080_0000_0000 + (205u64 << 30);
    assert_eq!(
        pml4_index(ANCHOR_VA),
        257,
        "sanity: anchor stays in slot 257"
    );
    assert_eq!(
        pml4_index(TARGET_VA),
        257,
        "sanity: target stays in slot 257"
    );

    vmm::unmap_virtual_address(ANCHOR_VA);
    let anchor_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("anchor frame alloc failed"));
    vmm::try_map_virtual_to_physical(ANCHOR_VA, anchor_frame.physical_address())
        .expect("anchor mapping should succeed");

    match walk_levels(TARGET_VA) {
        WalkResult::PdpMissing { pml4e } => {
            assert!(
                pml4e.present(),
                "PML4 entry must be present (anchor mapped)"
            );
        }
        _ => panic!("expected WalkResult::PdpMissing for an untouched PDP index"),
    }

    vmm::unmap_virtual_address(ANCHOR_VA);
}

/// Contract: walk_levels reports PdMissing when PML4+PDP are present but the
/// PD entry was never created.
/// Given: an anchor page is mapped at PDP index 210 / PD index 5 (forcing that
///        PDP entry to be present), and PD index 300 within the same PDP is
///        left untouched.
/// When: walk_levels is called for the untouched PD index.
/// Then: it returns WalkResult::PdMissing, carrying the resolved PML4/PDP entries.
#[test_case]
fn test_walk_levels_reports_pd_missing() {
    const PDP_BASE: u64 = 0xFFFF_8080_0000_0000 + (210u64 << 30);
    const ANCHOR_VA: u64 = PDP_BASE + (5u64 << 21);
    const TARGET_VA: u64 = PDP_BASE + (300u64 << 21);
    assert_eq!(pdp_index(ANCHOR_VA), 210, "sanity: anchor PDP index");
    assert_eq!(
        pdp_index(TARGET_VA),
        210,
        "sanity: target shares the same PDP"
    );
    assert_ne!(
        pd_index(ANCHOR_VA),
        pd_index(TARGET_VA),
        "sanity: distinct PDs"
    );

    vmm::unmap_virtual_address(ANCHOR_VA);
    let anchor_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("anchor frame alloc failed"));
    vmm::try_map_virtual_to_physical(ANCHOR_VA, anchor_frame.physical_address())
        .expect("anchor mapping should succeed");

    match walk_levels(TARGET_VA) {
        WalkResult::PdMissing { pml4e, pdpe } => {
            assert!(
                pml4e.present(),
                "PML4 entry must be present (anchor mapped)"
            );
            assert!(pdpe.present(), "PDP entry must be present (anchor mapped)");
            assert!(
                !pdpe.huge(),
                "anchor mapping is a 4 KiB page, PDP must not be huge"
            );
        }
        _ => panic!("expected WalkResult::PdMissing for an untouched PD index"),
    }

    vmm::unmap_virtual_address(ANCHOR_VA);
}

/// Contract: walk_levels resolves a fully-populated path down to the correct
/// leaf PT, matching the physical frame the caller just mapped.
#[test_case]
fn test_walk_levels_resolves_mapped_page() {
    const TEST_VA: u64 = 0xFFFF_8080_0000_0000 + (220u64 << 30);

    vmm::unmap_virtual_address(TEST_VA);
    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame alloc failed"));
    vmm::try_map_virtual_to_physical(TEST_VA, frame.physical_address())
        .expect("mapping should succeed");

    match walk_levels(TEST_VA) {
        WalkResult::Resolved {
            pml4e,
            pdpe,
            pde,
            pt,
        } => {
            assert!(pml4e.present() && !pml4e.huge());
            assert!(pdpe.present() && !pdpe.huge());
            assert!(pde.present() && !pde.huge());
            // SAFETY: `pt` is the leaf PT `walk_levels` just resolved for
            // `TEST_VA`, reached through the recursive self-mapping; `entries`
            // is a public field and `pt_index(TEST_VA) < PT_ENTRIES`.
            let pte = unsafe { (*pt).entries[pt_index(TEST_VA)] };
            assert!(pte.present(), "leaf PTE must be present");
            assert_eq!(
                pte.frame(),
                frame.pfn,
                "resolved leaf frame must match the mapped physical frame"
            );
        }
        _ => panic!("expected WalkResult::Resolved for a freshly mapped page"),
    }

    vmm::unmap_virtual_address(TEST_VA);
}

/// Contract: walk_levels reports PdpHuge (and does not touch the PD/PT below)
/// when the PDP entry is a present 1 GiB huge-page leaf.
/// Given: a normal 4 KiB mapping is created first (so PML4/PDP/PD/PT all
///        exist), then the PDP entry's huge bit is forced on directly through
///        the recursive mapping -- the kernel itself never creates huge
///        entries, so this uses raw bit manipulation solely to exercise the
///        walk's bail path; the bit is cleared again before teardown so the
///        hierarchy is left exactly as `unmap_virtual_address` expects it.
/// When: walk_levels is called while the huge bit is set.
/// Then: it returns WalkResult::PdpHuge without dereferencing a PD/PT address
///       computed from the (now huge) PDP entry's frame field.
#[test_case]
fn test_walk_levels_reports_pdp_huge() {
    const TEST_VA: u64 = 0xFFFF_8080_0000_0000 + (230u64 << 30);

    vmm::unmap_virtual_address(TEST_VA);
    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame alloc failed"));
    vmm::try_map_virtual_to_physical(TEST_VA, frame.physical_address())
        .expect("mapping should succeed");

    // Confirm the path is a normal, fully-resolved 4 KiB mapping before poking it.
    assert!(matches!(walk_levels(TEST_VA), WalkResult::Resolved { .. }));

    let pdp = pdp_table_addr(TEST_VA) as *mut PageTable;
    let idx = pdp_index(TEST_VA);
    // SAFETY:
    // - `pdp` is the live, currently-mapped PDP table for `TEST_VA` (just
    //   confirmed present/non-huge above), reached through the recursive
    //   self-mapping; `idx < PT_ENTRIES`.
    // - `PageTableEntry` is `#[repr(transparent)]` over `u64`, so reinterpreting
    //   the entry pointer as `*mut u64` to flip one bit is layout-compatible.
    // - This is test-only instrumentation: the huge bit is cleared again below
    //   before any other code path (including this test's own cleanup) touches
    //   the entry, so no other test observes the temporarily-huge entry.
    unsafe {
        let raw = entry_ptr(pdp, idx) as *mut u64;
        *raw |= ENTRY_HUGE;
    }

    match walk_levels(TEST_VA) {
        WalkResult::PdpHuge { pml4e, pdpe } => {
            assert!(pml4e.present());
            assert!(pdpe.present() && pdpe.huge());
        }
        _ => panic!("expected WalkResult::PdpHuge while the PDP huge bit is set"),
    }

    // SAFETY: same justification as above; this restores the entry to its
    // original (non-huge) state before the normal unmap path runs.
    unsafe {
        let raw = entry_ptr(pdp, idx) as *mut u64;
        *raw &= !ENTRY_HUGE;
    }
    vmm::unmap_virtual_address(TEST_VA);
}

/// Contract: walk_levels reports PdHuge (and does not touch the PT below)
/// when the PD entry is a present 2 MiB huge-page leaf.
/// Given/When/Then: mirrors `test_walk_levels_reports_pdp_huge` one level down.
#[test_case]
fn test_walk_levels_reports_pd_huge() {
    const TEST_VA: u64 = 0xFFFF_8080_0000_0000 + (240u64 << 30);

    vmm::unmap_virtual_address(TEST_VA);
    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame alloc failed"));
    vmm::try_map_virtual_to_physical(TEST_VA, frame.physical_address())
        .expect("mapping should succeed");

    assert!(matches!(walk_levels(TEST_VA), WalkResult::Resolved { .. }));

    let pd = pd_table_addr(TEST_VA) as *mut PageTable;
    let idx = pd_index(TEST_VA);
    // SAFETY: same justification as `test_walk_levels_reports_pdp_huge`, one
    // level down (PD instead of PDP).
    unsafe {
        let raw = entry_ptr(pd, idx) as *mut u64;
        *raw |= ENTRY_HUGE;
    }

    match walk_levels(TEST_VA) {
        WalkResult::PdHuge { pml4e, pdpe, pde } => {
            assert!(pml4e.present());
            assert!(pdpe.present() && !pdpe.huge());
            assert!(pde.present() && pde.huge());
        }
        _ => panic!("expected WalkResult::PdHuge while the PD huge bit is set"),
    }

    // SAFETY: restores the entry before the normal unmap path runs.
    unsafe {
        let raw = entry_ptr(pd, idx) as *mut u64;
        *raw &= !ENTRY_HUGE;
    }
    vmm::unmap_virtual_address(TEST_VA);
}

/// Contract: destroy_user_address_space's catch-all reclaim (`reclaim_user_range`,
/// refactored in issue #58 to resolve each present page's path once instead of
/// walking it twice) still finds and releases every present leaf mapping, even
/// when they are sparse and separated by both a 2 MiB (PD) and a 1 GiB (PDP)
/// unmapped gap that the skip-ahead logic must jump over correctly.
/// Given: three out-of-window pages are mapped directly at increasing
///        distances (0, +2 MiB, +1 GiB) from a base VA outside all three known
///        user regions (so only the generic catch-all scan can find them).
/// When: destroy_user_address_space tears down the address space.
/// Then: all three leaf frames are released back to the PMM.
/// Failure Impact: a regression in the skip-jump/resolve dispatch would leak
///       physical frames on every process exit -- exactly finding L1 warns about.
#[test_case]
fn test_destroy_user_address_space_reclaims_sparse_out_of_window_mappings() {
    const BASE_VA: u64 = vmm::USER_HEAP_END + 0x0010_0000;
    const FAR_VA_SAME_PDP: u64 = BASE_VA + 0x0020_0000; // +2 MiB: crosses a PD boundary
    const FAR_VA_NEXT_PDP: u64 = BASE_VA + 0x4000_0000; // +1 GiB: crosses a PDP boundary
    for va in [BASE_VA, FAR_VA_SAME_PDP, FAR_VA_NEXT_PDP] {
        assert!(
            vmm::classify_user_region(va).is_none(),
            "test VA must not fall inside any of the three known user regions"
        );
    }

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let frames: [_; 3] = [
        pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame 0 allocation failed")),
        pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame 1 allocation failed")),
        pmm::with_pmm(|mgr| mgr.alloc_frame().expect("frame 2 allocation failed")),
    ];
    let vas = [BASE_VA, FAR_VA_SAME_PDP, FAR_VA_NEXT_PDP];

    vmm::with_address_space(user_cr3, || {
        for (va, frame) in vas.iter().zip(frames.iter()) {
            vmm::try_map_virtual_to_physical(*va, frame.physical_address())
                .expect("mapping an out-of-window VA should still succeed at the page-table level");
        }
    });

    vmm::destroy_user_address_space(user_cr3);

    for frame in &frames {
        pmm::with_pmm(|mgr| {
            assert!(
                !mgr.release_pfn(frame.pfn),
                "teardown's catch-all pass must have already reclaimed every sparse mapping"
            );
        });
    }
}

/// Contract: `configure_uc_mapping` re-types an already-mapped page to strongly
/// uncacheable (PWT=1 + PCD=1) without disturbing its frame or its presence.
/// Failure Impact: this is the only way a driver can make an *already existing* mapping
/// uncacheable. `is_va_mapped` answers "is there a mapping", not "is it uncacheable", so
/// `drivers::ahci`'s ABAR setup depends on this to avoid driving its MMIO registers
/// write-back-cached when the #63 kernel-owned table mapped the aperture's region as
/// `EfiReservedMemoryType` (write-back, per design doc §4). Cached MMIO register writes can
/// sit in a cache line instead of reaching the device (#63 B4).
#[test_case]
fn test_configure_uc_mapping_retypes_existing_leaf() {
    use kaos_kernel::memory::vmm::page_table::{pt_table_addr, ENTRY_PCD, ENTRY_PWT};

    const TEST_VA: u64 = 0xFFFF_8000_0200_4000;

    // Start from a clean slate, then demand-map the page (write-back by default).
    vmm::unmap_virtual_address(TEST_VA);
    vmm::try_handle_page_fault(TEST_VA, 0).expect("demand mapping of TEST_VA must succeed");

    // Reads the leaf PTE through the recursive window; the fault above populated the
    // whole path, so every level exists.
    // SAFETY: `pt_table_addr(TEST_VA)` is the recursive-window VA of that leaf table, which
    // is mapped as long as TEST_VA is; entries are only read, never written.
    let leaf = |va: u64| unsafe {
        let pt = &*(pt_table_addr(va) as *const PageTable);
        pt.entries[pt_index(va)]
    };

    let before = leaf(TEST_VA);
    assert!(before.present(), "demand-mapped page must be present");
    assert_eq!(before.raw() & ENTRY_PCD, 0, "starts cacheable (PCD clear)");
    let frame_before = before.frame();

    vmm::configure_uc_mapping(TEST_VA, 4096);

    let after = leaf(TEST_VA);
    assert!(after.present(), "re-typing must not unmap the page");
    assert_eq!(
        after.frame(),
        frame_before,
        "re-typing must not move the frame"
    );
    assert_eq!(after.raw() & ENTRY_PCD, ENTRY_PCD, "PCD must be set");
    assert_eq!(
        after.raw() & ENTRY_PWT,
        ENTRY_PWT,
        "PWT must be set (strong UC, matching map_virtual_to_physical_uc)"
    );

    vmm::unmap_virtual_address(TEST_VA);
}
