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
    pmm::init(false);

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
/// Contract: pmm single allocation.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm single allocation".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_pmm_single_allocation() {
    pmm::with_pmm(|pmm| {
        let frame = pmm.alloc_frame();
        assert!(frame.is_some(), "Should be able to allocate a single frame");

        let frame = frame.unwrap();
        // PFN should be valid (greater than 0 since low memory is reserved)
        assert!(frame.pfn > 0, "PFN should be greater than 0");

        // Clean up
        assert!(pmm.release_pfn(frame.pfn));
    });
}

/// Test that multiple page frames can be allocated consecutively
/// Contract: pmm multiple allocations.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm multiple allocations".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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
                assert!(
                    pfns[i] != pfns[j],
                    "Each allocated frame should have a unique PFN"
                );
            }
        }

        // Clean up - release all frames
        assert!(pmm.release_pfn(frame0.pfn));
        assert!(pmm.release_pfn(frame1.pfn));
        assert!(pmm.release_pfn(frame2.pfn));
        assert!(pmm.release_pfn(frame3.pfn));
        assert!(pmm.release_pfn(frame4.pfn));
    });
}

/// Test that frames can be allocated and then released
/// Contract: pmm allocation and release.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm allocation and release".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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
        assert!(pmm.release_pfn(f0.pfn));
        assert!(pmm.release_pfn(f1.pfn));
        assert!(pmm.release_pfn(f2.pfn));

        // Verify frames were unique
        assert!(pfn0 != pfn1, "Frame 0 and 1 should have different PFNs");
        assert!(pfn1 != pfn2, "Frame 1 and 2 should have different PFNs");
        assert!(pfn0 != pfn2, "Frame 0 and 2 should have different PFNs");
    });
}

/// Contract: pmm self-test leaves PMM allocator usable for subsequent allocations.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm self-test leaves PMM allocator usable for subsequent allocations".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_pmm_self_test_leaves_allocator_usable() {
    pmm::run_self_test(64);

    pmm::with_pmm(|mgr| {
        let frame = mgr
            .alloc_frame()
            .expect("allocator must still return a frame after self-test");
        assert!(
            mgr.release_pfn(frame.pfn),
            "allocator must release frame after self-test"
        );
    });
}

/// Test that released frames are reused by subsequent allocations
/// Contract: pmm frame reuse after release.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm frame reuse after release".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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
        assert!(pmm.release_pfn(frame1.pfn));

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
        assert!(pmm.release_pfn(frame0.pfn));
        assert!(pmm.release_pfn(frame2.pfn));
        assert!(pmm.release_pfn(new_frame0.pfn));
        assert!(pmm.release_pfn(new_frame1.pfn));
        assert!(pmm.release_pfn(new_frame2.pfn));
    });
}

/// Test that no allocated frame falls within the reserved kernel region.
/// The PMM must mark [KERNEL_OFFSET, reserved_end) as used so that
/// alloc_frame() never hands out pages occupied by the kernel, stack,
/// or PMM metadata.
/// Contract: pmm reserved region not allocated.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm reserved region not allocated".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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

        for frame_slot in frames.iter_mut().take(NUM_FRAMES) {
            let frame = pmm.alloc_frame().expect("Allocation should succeed");
            let addr = frame.physical_address();

            assert!(
                !(KERNEL_OFFSET..STACK_TOP).contains(&addr),
                "Frame physical address 0x{:x} falls inside reserved region [0x{:x}, 0x{:x})",
                addr,
                KERNEL_OFFSET,
                STACK_TOP,
            );

            *frame_slot = frame.pfn;
            count += 1;

            // Return the frame immediately so we don't exhaust memory
            assert!(pmm.release_pfn(frame.pfn));
        }

        assert!(count == NUM_FRAMES, "All allocations should have succeeded");
    });
}

/// Test that physical_address() returns correct addresses
/// Contract: pmm physical address calculation.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "pmm physical address calculation".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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
        assert!(pmm.release_pfn(frame.pfn));
    });
}

/// Test that a single-owner (refcount == 1) frame is freed on exactly one
/// `release_pfn` call — the pre-existing allocator behavior must be
/// unaffected by the addition of per-frame refcounting.
/// Contract: pmm single owner frame freed after one release.
/// Given: A freshly allocated frame with no additional owners (refcount == 1).
/// When: `release_pfn` is called exactly once.
/// Then: The frame is returned to the free pool immediately, and a second
///       `release_pfn` call on the same PFN fails (already free).
/// Failure Impact: Regressing single-owner semantics would either leak every
///        frame in the kernel (never actually freed) or double-free on the
///        very first release call. Release-blocking.
#[test_case]
fn test_pmm_single_owner_frame_freed_after_one_release() {
    pmm::with_pmm(|mgr| {
        let free_before = mgr.total_free_frames();

        let frame = mgr.alloc_frame().expect("allocation should succeed");
        assert_eq!(
            mgr.total_free_frames(),
            free_before - 1,
            "allocating a frame must reduce the free count by exactly one"
        );

        assert!(
            mgr.release_pfn(frame.pfn),
            "releasing a single-owner frame must succeed"
        );
        assert_eq!(
            mgr.total_free_frames(),
            free_before,
            "a single-owner frame must be fully freed after exactly one release_pfn call"
        );

        assert!(
            !mgr.release_pfn(frame.pfn),
            "releasing an already-free frame a second time must fail"
        );
    });
}

/// Test that a frame with an extra owner (refcount == 2, via `inc_refcount`)
/// survives one `release_pfn` call and is only actually freed — and therefore
/// only re-allocatable — after the second `release_pfn` call.
///
/// This is the core new behavior the PMM frame-refcounting mechanism
/// introduces: it replaces the old manual boolean "does this caller own the
/// frame" policy with an accurate reference count, so a frame shared between
/// two owners (e.g. user-code aliased over another mapping) is never freed
/// while any owner still holds it, and is freed exactly once when the last
/// owner releases it.
/// Contract: pmm refcounted frame survives until last release.
/// Given: A frame allocated normally (refcount == 1), then shared with a
///        second owner via `inc_refcount` (refcount == 2).
/// When: `release_pfn` is called twice.
/// Then: The first call succeeds but leaves the frame allocated (free count
///       unchanged); the second call actually frees it (free count restored).
/// Failure Impact: A regression here would either free a still-shared frame
///        too early (double-use-after-free of physical memory across two
///        live mappings) or leak a frame forever once shared. Release-blocking.
#[test_case]
fn test_pmm_refcounted_frame_survives_until_last_release() {
    pmm::with_pmm(|mgr| {
        let free_before = mgr.total_free_frames();

        let frame = mgr.alloc_frame().expect("allocation should succeed");
        assert_eq!(mgr.total_free_frames(), free_before - 1);

        // Simulate a second owner aliasing this already-allocated frame.
        assert!(
            mgr.inc_refcount(frame.pfn),
            "inc_refcount must succeed on an allocated frame"
        );

        // First release: one owner (the second one) remains, so the frame
        // must stay allocated — the free count must not change.
        assert!(
            mgr.release_pfn(frame.pfn),
            "first release of a shared frame must succeed"
        );
        assert_eq!(
            mgr.total_free_frames(),
            free_before - 1,
            "a frame with a remaining owner must stay allocated after one release"
        );

        // Second release: last owner releases it, so the frame is now
        // actually freed and the free count is restored.
        assert!(
            mgr.release_pfn(frame.pfn),
            "second (final) release of a shared frame must succeed"
        );
        assert_eq!(
            mgr.total_free_frames(),
            free_before,
            "a shared frame must be freed once its last owner releases it"
        );

        // A third release on the now-free frame must fail.
        assert!(
            !mgr.release_pfn(frame.pfn),
            "releasing an already-free frame must fail"
        );
    });
}

/// Test that `inc_refcount` rejects a PFN that is not currently allocated,
/// both for a genuinely free frame and for a PFN outside every known region.
/// Contract: pmm inc refcount rejects unallocated or out of range pfn.
/// Given: A frame that was allocated and then fully released (free again),
///        and a PFN far outside any configured PMM region.
/// When: `inc_refcount` is called on each.
/// Then: Both calls return `false` — incrementing the refcount of a frame the
///       allocator does not consider allocated must never silently succeed
///       (it would create a phantom extra owner that can never be balanced).
/// Failure Impact: Silently accepting this would let a caller create a
///        reference to memory that isn't actually reserved by the allocator,
///        an invisible corruption/double-allocation hazard. Release-blocking.
#[test_case]
fn test_pmm_inc_refcount_rejects_unallocated_or_out_of_range_pfn() {
    pmm::with_pmm(|mgr| {
        let frame = mgr.alloc_frame().expect("allocation should succeed");
        assert!(mgr.release_pfn(frame.pfn), "initial release must succeed");

        assert!(
            !mgr.inc_refcount(frame.pfn),
            "inc_refcount must reject a PFN that is currently free"
        );

        assert!(
            !mgr.inc_refcount(u64::MAX / 2),
            "inc_refcount must reject a PFN outside every known region"
        );
    });
}
