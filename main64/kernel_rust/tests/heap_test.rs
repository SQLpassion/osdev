//! Heap Manager Integration Tests
//!
//! These tests verify basic heap allocation, reuse, and coalescing behavior.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use kaos_kernel::allocator::GLOBAL_ALLOCATOR;
use kaos_kernel::arch::interrupts;
use kaos_kernel::logging;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::sync::spinlock::SpinLock;

/// Entry point for the heap integration test kernel.
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

/// Contract: heap alloc free round trip.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap alloc free round trip".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_alloc_free_round_trip() {
    heap::init(false);
    let ptr = heap::malloc(16);
    assert!(!ptr.is_null(), "malloc should return non-null pointer");
    assert!(
        (ptr as usize).is_multiple_of(8),
        "heap allocation should be 8-byte aligned"
    );

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` is returned by `heap::malloc`, so it is valid and writable.
    // - We only access one byte within the allocated region.
    unsafe {
        core::ptr::write_volatile(ptr, 0xA5);
        let val = core::ptr::read_volatile(ptr);
        assert!(val == 0xA5, "heap memory should be writable/readable");
    }

    heap::free(ptr);
}

/// Contract: heap reuse after free.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap reuse after free".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_reuse_after_free() {
    heap::init(false);
    let ptr1 = heap::malloc(32);
    let ptr2 = heap::malloc(32);
    assert!(
        !ptr1.is_null() && !ptr2.is_null(),
        "allocations should succeed"
    );

    heap::free(ptr1);
    let ptr3 = heap::malloc(16);
    assert!(
        ptr3 == ptr1,
        "first-fit allocator should reuse the freed block"
    );

    heap::free(ptr2);
    heap::free(ptr3);
}

/// Contract: heap merge allows large alloc.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap merge allows large alloc".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_merge_allows_large_alloc() {
    heap::init(false);
    let ptr1 = heap::malloc(100);
    let ptr2 = heap::malloc(200);
    assert!(
        !ptr1.is_null() && !ptr2.is_null(),
        "allocations should succeed"
    );

    heap::free(ptr1);
    heap::free(ptr2);

    let ptr3 = heap::malloc(512);
    assert!(
        ptr3 == ptr1,
        "merged free blocks should satisfy larger allocation from heap start"
    );
    heap::free(ptr3);
}

/// Contract: heap coalesces both neighbors around a middle free.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap coalesces both neighbors around a middle free".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_coalesces_prev_and_next_neighbors() {
    heap::init(false);
    let ptr1 = heap::malloc(128);
    let ptr2 = heap::malloc(128);
    let ptr3 = heap::malloc(128);
    assert!(
        !ptr1.is_null() && !ptr2.is_null() && !ptr3.is_null(),
        "allocations should succeed"
    );

    // Free outer blocks first, then free the middle block to force two-sided coalescing.
    heap::free(ptr1);
    heap::free(ptr3);
    heap::free(ptr2);

    let merged = heap::malloc(320);
    assert!(
        merged == ptr1,
        "coalescing previous and next neighbors should produce one large block at first address"
    );
    heap::free(merged);
}

/// Contract: heap alignment for small allocs.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap alignment for small allocs".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_alignment_for_small_allocs() {
    heap::init(false);
    let ptr1 = heap::malloc(1);
    let ptr2 = heap::malloc(7);
    let ptr3 = heap::malloc(8);

    assert!(
        !ptr1.is_null() && !ptr2.is_null() && !ptr3.is_null(),
        "allocations should succeed"
    );
    assert!(
        (ptr1 as usize).is_multiple_of(8),
        "ptr1 should be 8-byte aligned"
    );
    assert!(
        (ptr2 as usize).is_multiple_of(8),
        "ptr2 should be 8-byte aligned"
    );
    assert!(
        (ptr3 as usize).is_multiple_of(8),
        "ptr3 should be 8-byte aligned"
    );

    heap::free(ptr1);
    heap::free(ptr2);
    heap::free(ptr3);
}

/// Contract: heap large allocation requires growth.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap large allocation requires growth".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_large_allocation_requires_growth() {
    heap::init(false);
    let ptr = heap::malloc(4096);
    assert!(
        !ptr.is_null(),
        "large allocation should succeed after heap growth"
    );
    assert!(
        (ptr as usize).is_multiple_of(8),
        "large allocation should be 8-byte aligned"
    );

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` is returned by `heap::malloc(4096)`, so 4096 bytes are valid.
    // - We only touch the last byte within that allocation.
    unsafe {
        core::ptr::write_volatile(ptr.add(4095), 0x5A);
        let val = core::ptr::read_volatile(ptr.add(4095));
        assert!(val == 0x5A, "large allocation should be writable/readable");
    }

    heap::free(ptr);
}

/// Contract: heap large allocation requires multiple growth steps.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap large allocation requires multiple growth steps".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_large_allocation_requires_multiple_growth_steps() {
    heap::init(false);
    let ptr = heap::malloc(9000);
    assert!(
        !ptr.is_null(),
        "large allocation should succeed after multiple heap growth steps"
    );
    assert!(
        (ptr as usize).is_multiple_of(8),
        "large allocation should be 8-byte aligned"
    );

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` is returned by `heap::malloc(9000)`, so 9000 bytes are valid.
    // - We only touch the last byte within that allocation.
    unsafe {
        core::ptr::write_volatile(ptr.add(8999), 0x3C);
        let val = core::ptr::read_volatile(ptr.add(8999));
        assert!(val == 0x3C, "large allocation should be writable/readable");
    }

    heap::free(ptr);
}

/// Contract: heap overflow request returns null and heap remains usable.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap overflow request returns null and heap remains usable".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_overflow_request_returns_null_and_heap_remains_usable() {
    heap::init(false);
    let overflow_ptr = heap::malloc(usize::MAX);
    assert!(
        overflow_ptr.is_null(),
        "overflow-size allocation should fail with null pointer"
    );

    let ptr = heap::malloc(32);
    assert!(
        !ptr.is_null(),
        "heap should remain usable after rejected overflow request"
    );
    heap::free(ptr);
}

/// Contract: heap rejects invalid free and remains usable.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap rejects invalid free and remains usable".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_rejects_invalid_free_and_remains_usable() {
    heap::init(false);
    let ptr = heap::malloc(64);
    assert!(!ptr.is_null(), "allocation should succeed");

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `ptr` points to a valid allocation.
    // - `ptr.add(1)` intentionally creates an invalid payload pointer for `free`.
    unsafe {
        heap::free(ptr.add(1));
    }

    // Original pointer must still be valid and free-able after rejected invalid free.
    heap::free(ptr);
    let ptr2 = heap::malloc(64);
    assert!(
        ptr2 == ptr,
        "heap should still be consistent after invalid free"
    );
    heap::free(ptr2);
}

/// Contract: heap rejects free when header magic is corrupted.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap rejects free when header magic is corrupted".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_rejects_free_with_corrupted_header_magic() {
    heap::init(false);
    let ptr1 = heap::malloc(64);
    let ptr2 = heap::malloc(64);
    assert!(!ptr1.is_null() && !ptr2.is_null(), "allocations should succeed");

    // Step 1: Infer header size from two adjacent equal-size allocations.
    let block_stride = (ptr2 as usize).saturating_sub(ptr1 as usize);
    let header_size = block_stride.saturating_sub(64);
    assert!(
        header_size >= 3 * core::mem::size_of::<usize>() && header_size.is_multiple_of(8),
        "derived header size should include size/prev/magic fields and alignment"
    );

    // Step 2: Corrupt the first block header magic so `free(ptr1)` must be rejected.
    let header_addr = (ptr1 as usize).saturating_sub(header_size);
    let magic_addr = header_addr + (2 * core::mem::size_of::<usize>());
    // SAFETY:
    // - This requires `unsafe` because raw pointer reads/writes are used directly.
    // - `header_addr` and `magic_addr` are derived from a live allocation header.
    // - We only toggle one bit in the magic word and restore it later in this test.
    let original_magic = unsafe {
        let magic_ptr = magic_addr as *mut usize;
        let value = core::ptr::read_volatile(magic_ptr);
        core::ptr::write_volatile(magic_ptr, value ^ 0x1);
        value
    };

    heap::free(ptr1);

    // Step 3: The corrupted block must still be considered allocated and not reusable.
    let ptr3 = heap::malloc(64);
    assert!(!ptr3.is_null(), "heap should remain usable after rejected free");
    assert!(
        ptr3 != ptr1,
        "corrupted header must prevent `ptr1` from being freed and reused"
    );

    // Step 4: Restore magic and verify the original block can be freed and reused again.
    // SAFETY:
    // - This requires `unsafe` because raw pointer writes are used directly.
    // - The same header location is still valid because `ptr1` was never freed.
    unsafe {
        core::ptr::write_volatile(magic_addr as *mut usize, original_magic);
    }

    heap::free(ptr1);
    let ptr4 = heap::malloc(64);
    assert!(
        ptr4 == ptr1,
        "restoring magic should allow normal free/reuse behavior again"
    );

    heap::free(ptr2);
    heap::free(ptr3);
    heap::free(ptr4);
}

/// Contract: heap rejects double free and remains usable.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap rejects double free and remains usable".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_rejects_double_free_and_remains_usable() {
    heap::init(false);
    let ptr = heap::malloc(64);
    assert!(!ptr.is_null(), "allocation should succeed");

    heap::free(ptr);
    heap::free(ptr);

    let ptr2 = heap::malloc(64);
    assert!(
        ptr2 == ptr,
        "double free should be rejected without corrupting heap state"
    );
    heap::free(ptr2);
}

/// Contract: heap self-test does not reset live allocator state.
#[test_case]
fn test_heap_self_test_is_non_destructive_for_live_allocations() {
    heap::init(false);
    let ptr = heap::malloc(64);
    assert!(
        !ptr.is_null(),
        "precondition: allocation before self-test must succeed"
    );

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` is a valid allocation returned by `heap::malloc(64)`.
    // - We read/write only one byte inside the allocation.
    unsafe {
        core::ptr::write_volatile(ptr, 0x5A);
    }

    let mut screen = kaos_kernel::drivers::screen::Screen::new();
    heap::run_self_test(&mut screen);

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - Pointer validity must survive the self-test (regression target).
    unsafe {
        let value = core::ptr::read_volatile(ptr);
        assert!(
            value == 0x5A,
            "self-test must not invalidate pre-existing allocations"
        );
    }

    heap::free(ptr);
}

/// Contract: heap growth is bounded and reports oom.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap growth is bounded and reports oom".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_growth_is_bounded_and_reports_oom() {
    heap::init(false);
    let ptr = heap::malloc(32 * 1024 * 1024);
    assert!(
        ptr.is_null(),
        "allocation beyond configured heap limit should fail"
    );

    let small = heap::malloc(64);
    assert!(
        !small.is_null(),
        "heap should remain usable after OOM failure"
    );
    heap::free(small);
}

/// Contract: heap debug output toggle round trip.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap debug output toggle round trip".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_debug_output_toggle_round_trip() {
    heap::init(false);
    assert!(
        !heap::debug_output_enabled(),
        "heap debug output should be disabled after init(false)"
    );

    let old = heap::set_debug_output(true);
    assert!(!old, "previous debug state should be false");
    assert!(
        heap::debug_output_enabled(),
        "heap debug output should now be enabled"
    );

    let old = heap::set_debug_output(false);
    assert!(old, "previous debug state should be true");
    assert!(
        !heap::debug_output_enabled(),
        "heap debug output should now be disabled"
    );
}

/// Contract: free-path logging with capture enabled remains allocator-safe.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "free-path logging with capture enabled remains allocator-safe".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_free_logging_with_capture_enabled_remains_allocator_safe() {
    heap::init(false);

    // Step 1: Force the free() diagnostics path to emit through serial + capture.
    let previous_debug = heap::set_debug_output(true);
    logging::set_capture_enabled(true);

    // Step 2: Allocate/free once to exercise the diagnostic path.
    let ptr = heap::malloc(96);
    assert!(
        !ptr.is_null(),
        "precondition: allocation before free-path diagnostic should succeed"
    );
    heap::free(ptr);

    // Step 3: Verify allocator remains usable after the logged free() operation.
    let ptr2 = heap::malloc(96);
    assert!(
        ptr2 == ptr,
        "free-path logging must not corrupt allocator state or deadlock progress"
    );
    heap::free(ptr2);

    // Step 4: Verify global allocator still works after the same path.
    let mut values = Vec::new();
    values.push(1_u8);
    assert!(
        values[0] == 1_u8,
        "global allocator should remain usable after logged free-path execution"
    );

    // Step 5: Restore logging settings for isolation with later tests.
    logging::set_capture_enabled(false);
    let _ = heap::set_debug_output(previous_debug);
}

/// Contract: heap preserves interrupt state when disabled.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "heap preserves interrupt state when disabled".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_heap_preserves_interrupt_state_when_disabled() {
    heap::init(false);
    interrupts::disable();
    assert!(
        !interrupts::are_enabled(),
        "interrupts should be disabled for this test"
    );

    let ptr = heap::malloc(16);
    heap::free(ptr);

    assert!(
        !interrupts::are_enabled(),
        "heap operations should not enable interrupts when they were disabled"
    );
}

/// Contract: spinlock basic mutation.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "spinlock basic mutation".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_spinlock_basic_mutation() {
    static LOCK: SpinLock<usize> = SpinLock::new(0);

    {
        let mut guard = LOCK.lock();
        *guard += 1;
    }

    let guard = LOCK.lock();
    assert!(*guard == 1, "spinlock should protect shared state");
}

/// Contract: global allocator round trip.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "global allocator round trip".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_global_allocator_round_trip() {
    heap::init(false);
    let layout = Layout::from_size_align(32, 8).unwrap();

    // SAFETY:
    // - This requires `unsafe` because manual allocator calls require upholding allocation and layout contracts that Rust cannot enforce.
    // - `layout` has non-zero size and valid alignment.
    // - Global allocator is initialized via `heap::init(false)` above.
    let ptr = unsafe { GLOBAL_ALLOCATOR.alloc(layout) };
    assert!(!ptr.is_null(), "global allocator should return a pointer");

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` was allocated with at least 32 bytes.
    // - We only touch the first byte of the allocation.
    unsafe {
        core::ptr::write_volatile(ptr, 0xCC);
        let val = core::ptr::read_volatile(ptr);
        assert!(
            val == 0xCC,
            "global allocator memory should be readable/writable"
        );
        GLOBAL_ALLOCATOR.dealloc(ptr, layout);
    }
}

/// Contract: global allocator supports overaligned layout.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "global allocator supports overaligned layout".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_global_allocator_supports_overaligned_layout() {
    heap::init(false);
    let layout = Layout::from_size_align(64, 64).unwrap();
    // SAFETY:
    // - This requires `unsafe` because manual allocator calls require upholding allocation and layout contracts that Rust cannot enforce.
    // - `layout` has non-zero size and power-of-two alignment.
    // - Global allocator is initialized via `heap::init(false)` above.
    let ptr = unsafe { GLOBAL_ALLOCATOR.alloc(layout) };
    assert!(!ptr.is_null(), "over-aligned allocation should succeed");
    assert!(
        (ptr as usize).is_multiple_of(64),
        "over-aligned allocation should satisfy requested alignment"
    );

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` was allocated for 64 bytes with alignment 64.
    // - Access stays within allocation bounds.
    unsafe {
        core::ptr::write_volatile(ptr, 0xAA);
        core::ptr::write_volatile(ptr.add(63), 0xBB);
        let first = core::ptr::read_volatile(ptr);
        let last = core::ptr::read_volatile(ptr.add(63));
        assert!(
            first == 0xAA && last == 0xBB,
            "memory should be readable/writable"
        );
        GLOBAL_ALLOCATOR.dealloc(ptr, layout);
    }
}

/// Contract: rust vec uses kernel heap.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "rust vec uses kernel heap".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_rust_vec_uses_kernel_heap() {
    heap::init(false);

    let mut values: Vec<u64> = Vec::with_capacity(16);
    for i in 0..16u64 {
        values.push(i * 3);
    }

    assert!(values.len() == 16, "Vec should contain 16 elements");
    assert!(values[0] == 0, "first Vec element should match");
    assert!(values[15] == 45, "last Vec element should match");
}
