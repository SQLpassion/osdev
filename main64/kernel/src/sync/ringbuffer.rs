//! Lock-free ring buffer with SPMC (single producer, multiple consumer) support.
//!
//! This module provides a fixed-size, generic byte ring buffer suitable for
//! interrupt-safe producer/consumer communication in a kernel environment.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Lock-free ring buffer for byte-oriented producer/consumer communication.
///
/// The buffer supports **single producer, multiple consumer (SPMC)** access:
///
/// - `push()` is safe for exactly **one** producer (no CAS needed because only
///   one writer ever advances `head_producer`).
/// - `pop()` is safe for **multiple** concurrent consumers.  It uses a
///   compare-and-swap (CAS) loop on `tail_consumer` so that two tasks calling
///   `pop()` at the same time never receive the same element.
///
/// This matters when wake-all semantics are used: if *all* waiting consumer
/// tasks are woken and race to call `pop()`, without CAS the plain `load →
/// read → store` sequence in `pop()` would allow two consumers to read the
/// same `tail_consumer` index and both obtain the same byte (duplicate
/// delivery).
///
/// ## Layout
///
/// Both `head_producer` and `tail_consumer` advance with **modular
/// arithmetic** (`% N`), wrapping from the last slot back to index 0.  This
/// makes the fixed-size array reusable in a cycle — hence "ring" buffer:
///
/// ```text
///    tail_consumer      head_producer
///         ▼                   ▼
///   ┌───┬───┬───┬───┬───┬───┬───┬───┐
///   │   │ A │ B │ C │ D │   │   │   │   N = 8
///   └───┴───┴───┴───┴───┴───┴───┴───┘
///         ▲               ▲
///       pop()           push()
///    (consumers)      (producer)
/// ```
///
/// After enough push/pop operations both indices can wrap around so that
/// `head_producer` is at a lower array index than `tail_consumer`:
///
/// ```text
///          head_producer   tail_consumer
///               ▼             ▼
///   ┌───┬───┬───┬───┬───┬───┬───┬───┐
///   │ X │ Y │   │   │   │   │ W │   │   N = 8
///   └───┴───┴───┴───┴───┴───┴───┴───┘
///   ← wrapped                  oldest
/// ```
///
/// - `head_producer` — **write index** (producer side).  Points to the next
///   free slot where `push()` will store a byte.  Advanced by the single
///   producer after writing.
/// - `tail_consumer` — **read index** (consumer side).  Points to the oldest
///   unread byte that `pop()` will return.  Advanced atomically (CAS) by
///   whichever consumer successfully claims the slot.
/// - The buffer is **empty** when `tail_consumer == head_producer` and **full**
///   when `(head_producer + 1) % N == tail_consumer` (one slot is always left
///   unused to distinguish full from empty).
pub struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    /// Write index (producer side): points to the next free slot.
    head_producer: AtomicUsize,
    /// Read index (consumer side): points to the oldest unread byte.
    tail_consumer: AtomicUsize,
}

impl<const N: usize> RingBuffer<N> {
    pub const fn new() -> Self {
        // Step 1: Initialize the buffer with zero bytes.
        // Step 2: Initialize both producer head and consumer tail to 0.
        Self {
            buf: UnsafeCell::new([0; N]),
            head_producer: AtomicUsize::new(0),
            tail_consumer: AtomicUsize::new(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        // Step 1: Perform acquire loads on both indices to check if they are equal.
        // If tail equals head, there are no unread bytes in the buffer.
        self.tail_consumer.load(Ordering::Acquire) == self.head_producer.load(Ordering::Acquire)
    }

    pub fn clear(&self) {
        // Step 1: Reset both indices to 0 using relaxed stores, since there are no ordering
        // constraints required for clearing/invalidating the buffer.
        self.head_producer.store(0, Ordering::Relaxed);
        self.tail_consumer.store(0, Ordering::Relaxed);
    }

    /// Append a byte to the buffer.  Returns `true` on success, `false` if
    /// the buffer is full (the byte is silently dropped in that case).
    ///
    /// Only safe for a **single producer** — no CAS is needed because exactly
    /// one writer ever advances `head_producer`.  The `Release` store on
    /// `head_producer` ensures that the byte written to `buf[head_producer]` is
    /// visible to any consumer that later observes the new `head_producer`
    /// value via an `Acquire` load.
    pub fn push(&self, value: u8) -> bool {
        // Step 1: Load current head and calculate next prospective index using modulo N.
        let head = self.head_producer.load(Ordering::Relaxed);
        let next = (head + 1) % N;

        // Step 2: Load current tail with Acquire ordering to observe consumer updates.
        let tail = self.tail_consumer.load(Ordering::Acquire);

        // Step 3: Check if the buffer is full. If the next index equals the tail,
        // we cannot write because one slot is left empty to distinguish full vs empty.
        if next == tail {
            return false;
        }

        // Step 4: Write the byte *before* publishing the new head_producer.
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Single-producer contract guarantees exclusive writes to `head` slot.
        // - `head` is in-bounds due to modulo arithmetic.
        unsafe {
            (*self.buf.get())[head] = value;
        }

        // Step 5: Publish the new head index using Release ordering so consumers can safely read the byte.
        self.head_producer.store(next, Ordering::Release);
        true
    }

    /// Remove and return the next element, or `None` if the buffer is empty.
    ///
    /// Uses a **CAS loop** to safely support multiple concurrent consumers:
    ///
    /// 1. Load the current `tail_consumer` index.
    /// 2. If `tail_consumer == head_producer` the buffer is empty → return
    ///    `None`.
    /// 3. Speculatively read the byte at `buf[tail_consumer]`.
    /// 4. Attempt to atomically advance `tail_consumer` via
    ///    `compare_exchange_weak`.
    ///    - **Success**: no other consumer changed `tail_consumer` between
    ///      step 1 and 4, so our read is valid → return the byte.
    ///    - **Failure**: another consumer already advanced `tail_consumer` (it
    ///      consumed the same slot first) → loop back to step 1 and retry with
    ///      the updated `tail_consumer` value.
    ///
    /// The speculative read in step 3 is safe because the producer only writes
    /// to `buf[head_producer]` *before* advancing `head_producer` with
    /// `Release` ordering, and we read `head_producer` with `Acquire` in
    /// step 2.  A slot between `tail_consumer` and `head_producer` is therefore
    /// always initialised and stable.
    pub fn pop(&self) -> Option<u8> {
        loop {
            // Step 1: Load both indices with Acquire ordering.
            let tail = self.tail_consumer.load(Ordering::Acquire);
            let head = self.head_producer.load(Ordering::Acquire);

            // Step 2: If the tail is equal to the head, the buffer is empty.
            if tail == head {
                return None;
            }

            // Step 3: Speculatively read the byte from the slot.
            // SAFETY:
            // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
            // - `tail != head` guarantees slot is initialized by producer.
            // - `tail` is in-bounds due to modulo arithmetic.
            let value = unsafe { (*self.buf.get())[tail] };
            let next = (tail + 1) % N;

            // Step 4: Attempt to transition tail_consumer from `tail` to `next` atomically.
            // If another consumer changed tail_consumer in the meantime, CAS fails, and we loop back.
            match self.tail_consumer.compare_exchange_weak(
                tail,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(value),
                Err(_) => continue,
            }
        }
    }
}

impl<const N: usize> Default for RingBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: All mutable access to `buf` is synchronized via atomic indices —
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// the producer writes only to `buf[head_producer]` before publishing, and
// consumers read only slots between `tail_consumer` and `head_producer`.
unsafe impl<const N: usize> Sync for RingBuffer<N> {}
// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - Sending the ring buffer transfers ownership of the atomic state and buffer.
// - Thread safety invariants are upheld by SPMC protocol and atomic indices.
unsafe impl<const N: usize> Send for RingBuffer<N> {}
