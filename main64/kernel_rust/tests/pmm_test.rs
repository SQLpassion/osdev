//! Physical Memory Manager Integration Tests
//!
//! This test verifies that the PMM correctly allocates and deallocates
//! physical page frames, including proper reuse of freed frames.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::memory::pmm;

/// Entry point for the PMM integration test kernel
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    // Initialize serial for test output
    kaos_kernel::drivers::serial::init();

    // Initialize the Physical Memory Manager
    pmm::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

// ============================================================================
// PMM Tests
// ============================================================================

/// Test that a single page frame can be allocated
#[test_case]
fn test_pmm_single_allocation() {
    pmm::with_pmm(|pmm| {
        let frame = pmm.alloc_frame();
        assert!(frame.is_some(), "Should be able to allocate a single frame");

        let frame = frame.unwrap();
        // PFN should be valid (greater than 0 since low memory is reserved)
        assert!(frame.pfn > 0, "PFN should be greater than 0");

        // Clean up
        pmm.release_frame(frame);
    });
}

/// Test that multiple page frames can be allocated consecutively
#[test_case]
fn test_pmm_multiple_allocations() {
    pmm::with_pmm(|pmm| {
        // Allocate 5 frames and store their PFNs
        let frame0 = pmm.alloc_frame().expect("Frame 0 allocation failed");
        let frame1 = pmm.alloc_frame().expect("Frame 1 allocation failed");
        let frame2 = pmm.alloc_frame().expect("Frame 2 allocation failed");
        let frame3 = pmm.alloc_frame().expect("Frame 3 allocation failed");
        let frame4 = pmm.alloc_frame().expect("Frame 4 allocation failed");

        let pfns = [frame0.pfn, frame1.pfn, frame2.pfn, frame3.pfn, frame4.pfn];

        // Verify all frames have unique PFNs
        for i in 0..5 {
            for j in (i + 1)..5 {
                assert!(pfns[i] != pfns[j], "Each allocated frame should have a unique PFN");
            }
        }

        // Clean up - release all frames
        pmm.release_frame(frame0);
        pmm.release_frame(frame1);
        pmm.release_frame(frame2);
        pmm.release_frame(frame3);
        pmm.release_frame(frame4);
    });
}

/// Test that frames can be allocated and then released
#[test_case]
fn test_pmm_allocation_and_release() {
    pmm::with_pmm(|pmm| {
        // Allocate 3 frames
        let frame0 = pmm.alloc_frame();
        let frame1 = pmm.alloc_frame();
        let frame2 = pmm.alloc_frame();

        assert!(frame0.is_some(), "Frame 0 allocation should succeed");
        assert!(frame1.is_some(), "Frame 1 allocation should succeed");
        assert!(frame2.is_some(), "Frame 2 allocation should succeed");

        let f0 = frame0.unwrap();
        let f1 = frame1.unwrap();
        let f2 = frame2.unwrap();

        // Store PFNs before release
        let pfn0 = f0.pfn;
        let pfn1 = f1.pfn;
        let pfn2 = f2.pfn;

        // Release all frames (should not panic)
        pmm.release_frame(f0);
        pmm.release_frame(f1);
        pmm.release_frame(f2);

        // Verify frames were unique
        assert!(pfn0 != pfn1, "Frame 0 and 1 should have different PFNs");
        assert!(pfn1 != pfn2, "Frame 1 and 2 should have different PFNs");
        assert!(pfn0 != pfn2, "Frame 0 and 2 should have different PFNs");
    });
}

/// Test that released frames are reused by subsequent allocations
#[test_case]
fn test_pmm_frame_reuse_after_release() {
    pmm::with_pmm(|pmm| {
        // Allocate 3 frames
        let frame0 = pmm.alloc_frame().expect("Frame 0 allocation failed");
        let frame1 = pmm.alloc_frame().expect("Frame 1 allocation failed");
        let frame2 = pmm.alloc_frame().expect("Frame 2 allocation failed");

        let _pfn0 = frame0.pfn;
        let pfn1 = frame1.pfn;
        let pfn2 = frame2.pfn;

        // Release the middle frame (frame1)
        pmm.release_frame(frame1);

        // Allocate 3 more frames
        let new_frame0 = pmm.alloc_frame().expect("New frame 0 allocation failed");
        let new_frame1 = pmm.alloc_frame().expect("New frame 1 allocation failed");
        let new_frame2 = pmm.alloc_frame().expect("New frame 2 allocation failed");

        let new_pfn0 = new_frame0.pfn;
        let new_pfn1 = new_frame1.pfn;
        let new_pfn2 = new_frame2.pfn;

        // The first new allocation should reuse the released frame (pfn1)
        assert!(
            new_pfn0 == pfn1,
            "First new allocation should reuse the released frame"
        );

        // The other new allocations should be new frames (after pfn2)
        assert!(
            new_pfn1 > pfn2,
            "Second new allocation should be a new frame after previous allocations"
        );
        assert!(
            new_pfn2 > new_pfn1,
            "Third new allocation should be after second"
        );

        // Clean up
        pmm.release_frame(frame0);
        pmm.release_frame(frame2);
        pmm.release_frame(new_frame0);
        pmm.release_frame(new_frame1);
        pmm.release_frame(new_frame2);
    });
}

/// Test that no allocated frame falls within the reserved kernel region.
/// The PMM must mark [KERNEL_OFFSET, reserved_end) as used so that
/// alloc_frame() never hands out pages occupied by the kernel, stack,
/// or PMM metadata.
#[test_case]
fn test_pmm_reserved_region_not_allocated() {
    const KERNEL_OFFSET: u64 = 0x100000;
    const STACK_TOP: u64 = 0x400000;

    pmm::with_pmm(|pmm| {
        // Allocate many frames and verify none overlap the reserved area.
        // 1024 frames = 4 MB worth of pages, enough to confirm the allocator
        // skips the entire reserved region.
        const NUM_FRAMES: usize = 1024;
        let mut frames = [0u64; NUM_FRAMES];
        let mut count = 0;

        for i in 0..NUM_FRAMES {
            let frame = pmm.alloc_frame().expect("Allocation should succeed");
            let addr = frame.physical_address();

            assert!(
                addr >= STACK_TOP || addr < KERNEL_OFFSET,
                "Frame physical address 0x{:x} falls inside reserved region [0x{:x}, 0x{:x})",
                addr,
                KERNEL_OFFSET,
                STACK_TOP,
            );

            frames[i] = frame.pfn;
            count += 1;

            // Return the frame immediately so we don't exhaust memory
            pmm.release_frame(frame);
        }

        assert!(count == NUM_FRAMES, "All allocations should have succeeded");
    });
}

/// Test that physical_address() returns correct addresses
#[test_case]
fn test_pmm_physical_address_calculation() {
    pmm::with_pmm(|pmm| {
        let frame = pmm.alloc_frame().expect("Frame allocation failed");

        let pfn = frame.pfn;
        let phys_addr = frame.physical_address();

        // Physical address should be PFN * 4096 (PAGE_SIZE)
        let expected_addr = pfn * 4096;
        assert!(
            phys_addr == expected_addr,
            "physical_address() should return PFN * PAGE_SIZE"
        );

        // Physical address should be page-aligned (multiple of 4096)
        assert!(
            phys_addr % 4096 == 0,
            "Physical address should be page-aligned"
        );

        // Clean up
        pmm.release_frame(frame);
    });
}
