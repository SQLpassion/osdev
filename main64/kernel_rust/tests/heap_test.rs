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
use core::panic::PanicInfo;
use core::alloc::{GlobalAlloc, Layout};
use kaos_kernel::allocator::GLOBAL_ALLOCATOR;
use kaos_kernel::arch::interrupts;
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
    heap::init();

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
fn test_heap_alloc_free_round_trip() {
    heap::init();
    let ptr = heap::malloc(16);
    assert!(!ptr.is_null(), "malloc should return non-null pointer");
    assert!(
        (ptr as usize) % 8 == 0,
        "heap allocation should be 8-byte aligned"
    );

    // SAFETY:
    // - `ptr` is returned by `heap::malloc`, so it is valid and writable.
    // - We only access one byte within the allocated region.
    unsafe {
        core::ptr::write_volatile(ptr, 0xA5);
        let val = core::ptr::read_volatile(ptr);
        assert!(val == 0xA5, "heap memory should be writable/readable");
    }

    heap::free(ptr);
}

#[test_case]
fn test_heap_reuse_after_free() {
    heap::init();
    let ptr1 = heap::malloc(32);
    let ptr2 = heap::malloc(32);
    assert!(!ptr1.is_null() && !ptr2.is_null(), "allocations should succeed");

    heap::free(ptr1);
    let ptr3 = heap::malloc(16);
    assert!(
        ptr3 == ptr1,
        "first-fit allocator should reuse the freed block"
    );

    heap::free(ptr2);
    heap::free(ptr3);
}

#[test_case]
fn test_heap_merge_allows_large_alloc() {
    heap::init();
    let ptr1 = heap::malloc(100);
    let ptr2 = heap::malloc(200);
    assert!(!ptr1.is_null() && !ptr2.is_null(), "allocations should succeed");

    heap::free(ptr1);
    heap::free(ptr2);

    let ptr3 = heap::malloc(512);
    assert!(
        ptr3 == ptr1,
        "merged free blocks should satisfy larger allocation from heap start"
    );
    heap::free(ptr3);
}

#[test_case]
fn test_heap_alignment_for_small_allocs() {
    heap::init();
    let ptr1 = heap::malloc(1);
    let ptr2 = heap::malloc(7);
    let ptr3 = heap::malloc(8);

    assert!(!ptr1.is_null() && !ptr2.is_null() && !ptr3.is_null(), "allocations should succeed");
    assert!((ptr1 as usize) % 8 == 0, "ptr1 should be 8-byte aligned");
    assert!((ptr2 as usize) % 8 == 0, "ptr2 should be 8-byte aligned");
    assert!((ptr3 as usize) % 8 == 0, "ptr3 should be 8-byte aligned");

    heap::free(ptr1);
    heap::free(ptr2);
    heap::free(ptr3);
}

#[test_case]
fn test_heap_large_allocation_requires_growth() {
    heap::init();
    let ptr = heap::malloc(4096);
    assert!(!ptr.is_null(), "large allocation should succeed after heap growth");
    assert!((ptr as usize) % 8 == 0, "large allocation should be 8-byte aligned");

    // SAFETY:
    // - `ptr` is returned by `heap::malloc(4096)`, so 4096 bytes are valid.
    // - We only touch the last byte within that allocation.
    unsafe {
        core::ptr::write_volatile(ptr.add(4095), 0x5A);
        let val = core::ptr::read_volatile(ptr.add(4095));
        assert!(val == 0x5A, "large allocation should be writable/readable");
    }

    heap::free(ptr);
}

#[test_case]
fn test_heap_preserves_interrupt_state_when_disabled() {
    heap::init();
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

#[test_case]
fn test_global_allocator_round_trip() {
    heap::init();
    let layout = Layout::from_size_align(32, 8).unwrap();

    let ptr = unsafe { GLOBAL_ALLOCATOR.alloc(layout) };
    assert!(!ptr.is_null(), "global allocator should return a pointer");

    // SAFETY:
    // - `ptr` was allocated with at least 32 bytes.
    // - We only touch the first byte of the allocation.
    unsafe {
        core::ptr::write_volatile(ptr, 0xCC);
        let val = core::ptr::read_volatile(ptr);
        assert!(val == 0xCC, "global allocator memory should be readable/writable");
        GLOBAL_ALLOCATOR.dealloc(ptr, layout);
    }
}

#[test_case]
fn test_rust_vec_uses_kernel_heap() {
    heap::init();

    let mut values: Vec<u64> = Vec::with_capacity(16);
    for i in 0..16u64 {
        values.push(i * 3);
    }

    assert!(values.len() == 16, "Vec should contain 16 elements");
    assert!(values[0] == 0, "first Vec element should match");
    assert!(values[15] == 45, "last Vec element should match");
}
