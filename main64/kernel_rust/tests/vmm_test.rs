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

    unsafe {
        let ptr = TEST_VA as *mut u8;
        core::ptr::write_volatile(ptr, 0x5A);
        let val = core::ptr::read_volatile(ptr);
        assert!(val == 0x5A, "mapped page should be writable after non-present fault");
    }

    vmm::unmap_virtual_address(TEST_VA);
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

    unsafe {
        let base = TEST_VA as *mut u8;
        core::ptr::write_volatile(base, 0xAB);
        core::ptr::write_volatile(base.add(4095), 0xCD);
    }

    vmm::unmap_virtual_address(TEST_VA);

    vmm::try_handle_page_fault(TEST_VA, 0)
        .expect("non-present fault should be handled by demand allocation");

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
    assert!(token == 0xC0DEu64, "closure return value must be propagated");

    assert!(
        vmm::get_pml4_address() == kernel_cr3,
        "with_address_space must restore the previous CR3 after closure returns"
    );

    pmm::with_pmm(|mgr| assert!(mgr.release_pfn(user_cr3 / pmm::PAGE_SIZE)));
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
