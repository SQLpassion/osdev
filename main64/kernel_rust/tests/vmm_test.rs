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

    pmm::init();
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

#[test_case]
fn test_vmm_smoke_once() {
    vmm::set_debug_output(true);
    assert!(vmm::test_vmm(), "vmm::test_vmm() should succeed");
    vmm::set_debug_output(false);
}

#[test_case]
fn test_vmm_smoke_twice() {
    vmm::set_debug_output(true);
    assert!(vmm::test_vmm(), "first vmm::test_vmm() run should succeed");
    assert!(vmm::test_vmm(), "second vmm::test_vmm() run should succeed");
    vmm::set_debug_output(false);
}

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
    pmm::with_pmm(|mgr| mgr.release_frame(frame));

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
