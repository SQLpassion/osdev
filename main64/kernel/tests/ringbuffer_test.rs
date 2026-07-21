//! Ring buffer integration tests.
//!
//! These tests verify the SPMC ring buffer contract, including the
//! free-running-counter fix for the ABA race described in R-15.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::sync::ringbuffer::RingBuffer;

/// Entry point for the ring buffer integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

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

/// Contract: basic push/pop across the ring buffer works.
/// Given: A freshly created ring buffer of capacity 4.
/// When: Three bytes are pushed and then popped.
/// Then: The bytes are returned in FIFO order and the buffer is empty again.
#[test_case]
fn test_ringbuffer_basic_fifo() {
    static mut BUFFER: RingBuffer<4> = RingBuffer::new();

    // SAFETY: This test runs on a single CPU with interrupts enabled but no
    // other task accesses the static buffer.
    let buf = unsafe { &*core::ptr::addr_of!(BUFFER) };

    assert!(buf.is_empty());

    assert!(buf.push(10));
    assert!(buf.push(20));
    assert!(buf.push(30));

    assert_eq!(buf.pop(), Some(10));
    assert_eq!(buf.pop(), Some(20));
    assert_eq!(buf.pop(), Some(30));
    assert_eq!(buf.pop(), None);
    assert!(buf.is_empty());
}

/// Contract: the ring buffer drops bytes when full.
/// Given: A ring buffer of capacity 2.
/// When: The buffer is filled completely and another byte is pushed.
/// Then: The overflow push fails, all existing bytes remain, and are returned
/// in FIFO order.
#[test_case]
fn test_ringbuffer_full_drops_new_byte() {
    static mut BUFFER: RingBuffer<2> = RingBuffer::new();

    // SAFETY: This test runs on a single CPU with no other task accessing the
    // static buffer.
    let buf = unsafe { &*core::ptr::addr_of!(BUFFER) };

    assert!(buf.push(1));
    assert!(buf.push(2));
    assert!(!buf.push(3));

    assert_eq!(buf.pop(), Some(1));
    assert_eq!(buf.pop(), Some(2));
    assert_eq!(buf.pop(), None);
}

/// Contract: free-running counters wrap around the array correctly.
/// Given: A ring buffer of capacity 4.
/// When: Exactly four bytes are pushed (filling the buffer), then all four are
/// popped, and the same slots are reused for another four bytes.
/// Then: The second batch is returned correctly; the counters are monotonic and
/// the modulo indexing works across the wrap point.
#[test_case]
fn test_ringbuffer_wrap_around() {
    static mut BUFFER: RingBuffer<4> = RingBuffer::new();

    // SAFETY: This test runs on a single CPU with no other task accessing the
    // static buffer.
    let buf = unsafe { &*core::ptr::addr_of!(BUFFER) };

    // Fill the buffer completely (capacity 4).
    for i in 0..4 {
        assert!(buf.push(i));
    }
    assert!(!buf.push(99));

    // Drain it.
    for i in 0..4 {
        assert_eq!(buf.pop(), Some(i));
    }
    assert!(buf.is_empty());

    // Reuse the wrapped-around slots with distinct values.
    for i in 4..8 {
        assert!(buf.push(i));
    }

    for i in 4..8 {
        assert_eq!(buf.pop(), Some(i));
    }
    assert_eq!(buf.pop(), None);
}

/// Contract: pop returns None from an empty buffer.
/// Given: A fresh ring buffer.
/// When: pop is called without any prior push.
/// Then: pop returns None.
#[test_case]
fn test_ringbuffer_pop_empty() {
    static mut BUFFER: RingBuffer<8> = RingBuffer::new();

    // SAFETY: This test runs on a single CPU with no other task accessing the
    // static buffer.
    let buf = unsafe { &*core::ptr::addr_of!(BUFFER) };

    assert_eq!(buf.pop(), None);
    assert!(buf.is_empty());
}

/// Contract: clear resets the ring buffer to empty.
/// Given: A ring buffer containing data.
/// When: clear is called.
/// Then: pop returns None and the buffer reports empty.
#[test_case]
fn test_ringbuffer_clear() {
    static mut BUFFER: RingBuffer<8> = RingBuffer::new();

    // SAFETY: This test runs on a single CPU with no other task accessing the
    // static buffer.
    let buf = unsafe { &*core::ptr::addr_of!(BUFFER) };

    assert!(buf.push(42));
    assert!(buf.push(43));
    buf.clear();

    assert!(buf.is_empty());
    assert_eq!(buf.pop(), None);
}
