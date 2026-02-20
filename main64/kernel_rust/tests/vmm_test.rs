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
use kaos_kernel::memory::{pmm, vmm};

/// Entry point for the VMM integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);

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

/// Contract: non present fault allocates and maps page.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "non present fault allocates and maps page".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_non_present_fault_allocates_and_maps_page() {
    const TEST_VA: u64 = 0xFFFF_8091_2345_6000;
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

/// Contract: user fault mappings keep user path bits set and code leaf read-only.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "user fault mappings keep user path bits set and code leaf read-only".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_user_fault_mapping_sets_user_bits_and_code_readonly_leaf() {
    let code_va = vmm::USER_CODE_BASE + 0x0011_5000;
    let stack_va = vmm::USER_STACK_TOP - 4096;

    vmm::unmap_virtual_address(code_va);
    vmm::unmap_virtual_address(stack_va);

    // Simulate non-present user faults (`U=1`, `P=0` -> error code 0x4).
    vmm::try_handle_page_fault(code_va, 0x4)
        .expect("user code non-present fault should be demand-mapped");
    vmm::try_handle_page_fault(stack_va, 0x4)
        .expect("user stack non-present fault should be demand-mapped");

    let code_flags = vmm::debug_mapping_flags_for_va(code_va)
        .expect("code VA should have present mapping flags");
    assert!(
        code_flags == (true, true, true, true, false),
        "code VA must have user path bits set and read-only leaf"
    );

    let stack_flags = vmm::debug_mapping_flags_for_va(stack_va)
        .expect("stack VA should have present mapping flags");
    assert!(
        stack_flags == (true, true, true, true, true),
        "stack VA must have user path bits set and writable leaf"
    );

    vmm::unmap_virtual_address(code_va);
    vmm::unmap_virtual_address(stack_va);
}

/// Contract: faulted page is zero initialized.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "faulted page is zero initialized".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_faulted_page_is_zero_initialized() {
    const TEST_VA: u64 = 0xFFFF_8092_3456_7000;
    vmm::unmap_virtual_address(TEST_VA);

    let frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("expected frame allocation for zero-init test")
    });
    vmm::map_virtual_to_physical(TEST_VA, frame.physical_address());

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `TEST_VA` is mapped writable to `frame` for one page.
    // - Access stays within first and last byte of that mapped page.
    unsafe {
        let base = TEST_VA as *mut u8;
        core::ptr::write_volatile(base, 0xAB);
        core::ptr::write_volatile(base.add(4095), 0xCD);
    }

    vmm::unmap_virtual_address(TEST_VA);

    vmm::try_handle_page_fault(TEST_VA, 0)
        .expect("non-present fault should be handled by demand allocation");

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
    const TEST_VA: u64 = 0xFFFF_8093_4567_8000;

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
            vmm::get_pml4_address() == user_cr3,
            "closure must observe target CR3 as active address space"
        );
        0xC0DEu64
    });
    assert!(
        token == 0xC0DEu64,
        "closure return value must be propagated"
    );

    assert!(
        vmm::get_pml4_address() == kernel_cr3,
        "with_address_space must restore the previous CR3 after closure returns"
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

/// Contract: destroy user address space does not release code leaf frame.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space does not release code leaf frame".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_does_not_release_code_leaf_frame() {
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
            mgr.release_pfn(code_leaf.pfn),
            "code-leaf PFN should remain allocated after destroy and require explicit release"
        );
    });
}

/// Contract: destroy user address space with owned-code policy releases code leaf frame.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space with owned-code policy releases code leaf frame".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_destroy_user_address_space_with_options_releases_code_leaf_frame() {
    const TEST_CODE_VA: u64 = vmm::USER_CODE_BASE;

    let user_cr3 = vmm::clone_kernel_pml4_for_user();
    let code_leaf = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("code frame allocation failed"));

    vmm::with_address_space(user_cr3, || {
        vmm::map_user_page(TEST_CODE_VA, code_leaf.pfn, false)
            .expect("test code VA should map in cloned address space");
    });

    vmm::destroy_user_address_space_with_options(user_cr3, true);

    pmm::with_pmm(|mgr| {
        assert!(
            !mgr.release_pfn(code_leaf.pfn),
            "owned-code policy must release code-leaf PFN during address-space destroy"
        );
    });
}

/// Contract: destroy user address space with page counts tears down only mapped code/stack pages.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "destroy user address space with page counts tears down only mapped code/stack pages".
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
    vmm::destroy_user_address_space_with_page_counts(user_cr3, true, 1, 1);

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

/// Contract: fault mapped code page has no execute bit clear.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "fault mapped code page has no execute bit clear".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_fault_mapped_code_page_has_no_execute_bit_clear() {
    // Code page demand-mapped via the page-fault handler path.
    let code_va = vmm::USER_CODE_BASE + 0x1000;
    vmm::unmap_virtual_address(code_va);

    // Simulate a non-present user-mode code fault (U=1, P=0 → error_code = 0x4).
    vmm::try_handle_page_fault(code_va, 0x4)
        .expect("user code non-present fault should be demand-mapped");

    // Code pages must remain executable: No-Execute bit must NOT be set.
    let nx = vmm::debug_no_execute_flag_for_va(code_va)
        .expect("demand-mapped code VA must have a present leaf PTE");
    assert!(
        !nx,
        "demand-mapped code leaf PTE must not have No-Execute bit set"
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
