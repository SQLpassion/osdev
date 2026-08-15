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
use kaos_kernel::memory::pmm::manager::{check_metadata_fits, select_metadata_base};
use kaos_kernel::memory::vmm::page_table::{self, PageTable};

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

/// Test that `mark_range_used` (exercised via the debug-only
/// `mark_range_used_for_test` accessor) is idempotent against overlapping and
/// repeated ranges — issue #52. Before the fix, every frame covered by more
/// than one reserved range had `frames_free` decremented once per covering
/// call, silently corrupting the free-frame count and, given enough overlap,
/// eventually underflowing it (`checked_sub().unwrap()` panic).
///
/// The range used here is chosen deep inside the first managed region (far
/// past `KERNEL_OFFSET`/`STACK_TOP` and any low-PFN churn from earlier tests
/// in this binary), so it is guaranteed to start out free regardless of test
/// execution order.
/// Contract: pmm mark_range_used is idempotent against overlapping ranges.
/// Given: A range of frames known to start out free.
/// When: The range is marked used, then an overlapping range is marked used,
///       then the original range is marked used again.
/// Then: `frames_free` drops by exactly the number of *distinct* frames ever
///       covered — never more, regardless of how many times a frame is
///       re-covered by a later call.
/// Failure Impact: Silent free-frame accounting corruption and an eventual
///        `checked_sub().unwrap()` panic if a future change (or a bootloader
///        supplying an overlapping metadata range) causes `mark_range_used`
///        to be called with overlapping ranges. Release-blocking per #52.
#[test_case]
fn test_mark_range_used_is_idempotent_for_overlapping_ranges() {
    pmm::with_pmm(|mgr| {
        let region_start = mgr
            .regions_snapshot()
            .first()
            .expect("PMM must have at least one region")
            .start;

        // Frame offsets A, B, C, D deep inside the region; A..C is the first
        // reservation, C..E is a second reservation overlapping it at frame C.
        let range_a_c_start = region_start + 5000 * pmm::PAGE_SIZE;
        let range_a_c_end = range_a_c_start + 3 * pmm::PAGE_SIZE; // covers A, B, C
        let range_c_e_start = range_a_c_start + 2 * pmm::PAGE_SIZE; // frame C
        let range_c_e_end = range_c_e_start + 2 * pmm::PAGE_SIZE; // covers C, D

        let free_before = mgr.total_free_frames();

        // First call: marks 3 new frames (A, B, C) used.
        mgr.mark_range_used_for_test(range_a_c_start, range_a_c_end);
        assert_eq!(
            mgr.total_free_frames(),
            free_before - 3,
            "first reservation must decrement frames_free by exactly the frames it covers"
        );

        // Second call overlaps at frame C: only D is a genuinely new frame,
        // so frames_free must drop by 1, not 2.
        mgr.mark_range_used_for_test(range_c_e_start, range_c_e_end);
        assert_eq!(
            mgr.total_free_frames(),
            free_before - 4,
            "an overlapping reservation must not double-decrement the shared frame"
        );

        // Repeating the very first range again must be a complete no-op: every
        // frame it covers (A, B, C) is already marked used.
        mgr.mark_range_used_for_test(range_a_c_start, range_a_c_end);
        assert_eq!(
            mgr.total_free_frames(),
            free_before - 4,
            "re-marking an already-reserved range must not change frames_free"
        );

        // Cleanup: these frames were never handed out via alloc_frame, so their
        // refcount byte is still 0. release_pfn's documented refcount==0
        // fallback treats a bitmap-set/refcount-0 frame as the "actually free"
        // case, clearing the bit and restoring frames_free.
        let start_pfn = range_a_c_start / pmm::PAGE_SIZE;
        for pfn in start_pfn..start_pfn + 4 {
            assert!(mgr.release_pfn(pfn), "cleanup release must succeed");
        }
        assert_eq!(
            mgr.total_free_frames(),
            free_before,
            "cleanup must restore the original free-frame baseline"
        );
    });
}

// ============================================================================
// #63 R1: `is_pfn_free` query + `assert_no_active_table_frame_is_pmm_free` guard.
//
// These back the invariant that makes skipping `reserve_firmware_page_tables` on the
// kernel-owned-table path safe (no live firmware/loader page-table frame may be a free
// PMM frame). See `docs/todo_uefi_kernel_pagetables.md` §R1. The *panic* case lives in
// `firmware_tables_pmm_pool_death_test.rs`.
// ============================================================================

/// Contract: `is_pfn_free` mirrors the allocation bitmap for managed frames and reports
/// `false` for PFNs outside every managed region.
/// Failure Impact: the R1 guard is built on this query; a wrong answer would either make
/// the guard miss a real violation or abort every boot with a false positive.
#[test_case]
fn test_is_pfn_free_tracks_bitmap_and_range() {
    pmm::with_pmm(|mgr| {
        // A just-allocated frame is used -> not free.
        let frame = mgr.alloc_frame().expect("allocation should succeed");
        assert!(
            !mgr.is_pfn_free(frame.pfn),
            "an allocated frame must not report free"
        );

        // Releasing it returns the very same PFN to the free pool.
        assert!(mgr.release_pfn(frame.pfn), "release must succeed");
        assert!(
            mgr.is_pfn_free(frame.pfn),
            "a released frame must report free again"
        );

        // PFN 0 is below KERNEL_OFFSET (1 MiB) and thus outside every managed region.
        assert!(
            !mgr.is_pfn_free(0),
            "an unmanaged low PFN must not report free"
        );

        // A PFN far past any RAM region is likewise unmanaged.
        assert!(
            !mgr.is_pfn_free(u64::MAX / 2),
            "an out-of-range PFN must not report free"
        );
    });
}

/// Contract: the #63 R1 guard walks a small table whose every frame is an allocated
/// (hence used) PMM frame and returns without panicking — it must not false-positive on
/// the common case (which is exactly what happens on every real boot, where the live
/// firmware tree's frames are all outside the PMM pool).
/// Failure Impact: a spurious panic here would abort every real boot, since the guard
/// runs on the live firmware tree in `switch_to_direct_map`. Release-blocking.
#[test_case]
fn test_r1_guard_passes_when_no_table_frame_is_free() {
    // Two allocated (used), zeroed frames: a root and one sub-table it points at, so the
    // guard actually descends a level rather than only checking the root.
    let root = page_table::alloc_frame_phys_or_panic("test: R1 guard root");
    let sub = page_table::alloc_frame_phys_or_panic("test: R1 guard sub");
    page_table::zero_phys_page(root);
    page_table::zero_phys_page(sub);

    // Point root[0] at the sub-table (present, non-huge).
    // SAFETY: `root` is a freshly allocated, identity-mapped low frame; writing its
    // first entry through the identity map is the same trick the direct-map tests use.
    unsafe {
        (*(root as *mut PageTable)).entries[0].set_mapping(sub >> 12, true, true, false);
    }

    // Both frames are used, so the guard must complete without panicking.
    // SAFETY: `root` and every frame reachable from it is identity-mapped and live.
    unsafe { page_table::assert_no_active_table_frame_is_pmm_free(root) };

    pmm::with_pmm(|mgr| {
        assert!(mgr.release_pfn(root >> 12), "root release must succeed");
        assert!(mgr.release_pfn(sub >> 12), "sub release must succeed");
    });
}

// ============================================================================
// PMM metadata-base selection tests — pure; no firmware, no allocation.
//
// `PhysicalMemoryManager::new()` must place its layout (header + region array + bitmaps)
// in the bootloader-reserved region (`BootInfo.pmm_metadata_base`) when one is provided,
// and otherwise fall back to "right after the kernel image/BSS". On large-RAM UEFI
// systems the bitmaps are far too big to sit in low memory, so picking the wrong base
// triple-faulted real hardware (see `docs/pmm.md` §2). That decision is factored into the
// pure helper `select_metadata_base`, which these tests pin directly — no BootInfo
// pointer, no `__bss_end`, no side effects.
// ============================================================================

/// A stand-in "address right after the kernel BSS" for the fallback cases.
const KERNEL_END_PHYS: u64 = 0x0020_0000;
/// A stand-in bootloader-reserved metadata base, far above low memory (UEFI-style).
const RESERVED_BASE: u64 = 0x0000_0020_0000_0000;

/// Contract: with a non-zero bootloader-reserved base, the PMM uses that base (UEFI path).
/// Failure Impact: the bitmaps would land in low memory and overrun firmware on large-RAM
///        hardware — the original triple-fault. Release-blocking.
#[test_case]
fn test_uses_reserved_base_when_present() {
    assert_eq!(
        select_metadata_base(Some(RESERVED_BASE), KERNEL_END_PHYS),
        RESERVED_BASE,
        "a non-zero pmm_metadata_base must win over the kernel-end fallback"
    );
}

/// Contract: with no BootInfo (BIOS loader / tests), the PMM falls back to the kernel end.
/// Failure Impact: the BIOS path would dereference a bogus base. Release-blocking.
#[test_case]
fn test_falls_back_when_no_boot_info() {
    assert_eq!(
        select_metadata_base(None, KERNEL_END_PHYS),
        KERNEL_END_PHYS,
        "absent BootInfo must fall back to the address after the kernel image"
    );
}

/// Contract: a BootInfo present but with `pmm_metadata_base == 0` also falls back.
/// (`0` is the loader's "no reserved region" sentinel — see `BootInfo` docs.)
/// Failure Impact: treating 0 as a real base would point the layout at the null page.
///        Release-blocking.
#[test_case]
fn test_falls_back_when_reserved_base_zero() {
    assert_eq!(
        select_metadata_base(Some(0), KERNEL_END_PHYS),
        KERNEL_END_PHYS,
        "a zero pmm_metadata_base is the 'not provided' sentinel and must fall back"
    );
}

/// Contract: check_metadata_fits returns true when metadata_end is within the reserved size.
/// Failure Impact: false positive assertion failures on valid loader-reserved sizes.
#[test_case]
fn test_check_metadata_fits_within_size() {
    let base = 0x0000_0020_0000_0000;
    let size = 0x10000; // 64 KiB
    assert!(
        check_metadata_fits(base + size, base, size),
        "metadata ending exactly at reserved boundary must fit"
    );
    assert!(
        check_metadata_fits(base + 0x1000, base, size),
        "metadata ending well within reserved size must fit"
    );
}

/// Contract: check_metadata_fits returns false when metadata_end exceeds the reserved size.
/// Failure Impact: PMM metadata overrunning loader-reserved region silently without assertion.
#[test_case]
fn test_check_metadata_fits_exceeds_size() {
    let base = 0x0000_0020_0000_0000;
    let size = 0x10000; // 64 KiB
    assert!(
        !check_metadata_fits(base + size + 1, base, size),
        "metadata ending past reserved size must return false"
    );
}
