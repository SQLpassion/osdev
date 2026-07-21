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
/// Both `head_producer` and `tail_consumer` are **free-running counters**
/// (`usize`) that never wrap back to zero explicitly.  The fixed-size array is
/// indexed with modulo `N`, which makes the storage reusable in a cycle —
/// hence "ring" buffer:
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
/// Using free-running counters prevents the ABA race that would otherwise be
/// possible with wrapped indices: a consumer that is preempted between reading
/// a slot and CAS-ing `tail_consumer` cannot succeed later if the counter has
/// advanced by `N` or more in the meantime, even though the modulo index
/// happens to be the same.
///
/// - `head_producer` — **write counter** (producer side).  Points to the next
///   free slot where `push()` will store a byte.  Advanced by the single
///   producer after writing.
/// - `tail_consumer` — **read counter** (consumer side).  Points to the oldest
///   unread byte that `pop()` will return.  Advanced atomically (CAS) by
///   whichever consumer successfully claims the slot.
/// - The buffer is **empty** when `tail_consumer == head_producer` and **full**
///   when `head_producer - tail_consumer == N`.
pub struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    /// Write counter (producer side): points to the next free slot.
    head_producer: AtomicUsize,
    /// Read counter (consumer side): points to the oldest unread byte.
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
        // Step 1: Perform acquire loads on both counters to check if they are equal.
        // If tail equals head, there are no unread bytes in the buffer.
        self.tail_consumer.load(Ordering::Acquire) == self.head_producer.load(Ordering::Acquire)
    }

    pub fn clear(&self) {
        // Step 1: Reset both counters to 0 using relaxed stores, since there are no ordering
        // constraints required for clearing/invalidating the buffer.
        self.head_producer.store(0, Ordering::Relaxed);
        self.tail_consumer.store(0, Ordering::Relaxed);
    }

    /// Append a byte to the buffer.  Returns `true` on success, `false` if
    /// the buffer is full (the byte is silently dropped in that case).
    ///
    /// Only safe for a **single producer** — no CAS is needed because exactly
    /// one writer ever advances `head_producer`.  The `Release` store on
    /// `head_producer` ensures that the byte written to `buf[head_producer]`
    /// is visible to any consumer that later observes the new
    /// `head_producer` value via an `Acquire` load.
    pub fn push(&self, value: u8) -> bool {
        // Step 1: Load current head and tail counters.
        // The counters are free-running; only the array index uses modulo N.
        let head = self.head_producer.load(Ordering::Relaxed);
        let tail = self.tail_consumer.load(Ordering::Acquire);

        // Step 2: Check if the buffer is full.  Full when the producer has
        // advanced N slots ahead of the consumer.  Wrapping arithmetic is
        // required because the counters are monotonic and may wrap around
        // after extremely long runtime (64-bit wraparound is not reachable
        // in practice).
        if head.wrapping_sub(tail) == N {
            return false;
        }

        // Step 3: Write the byte *before* publishing the new head_producer.
        // SAFETY:
        // - Single-producer contract guarantees exclusive writes to the `head` slot.
        // - `head % N` is in-bounds because N equals the array length.
        // - No mutable reference to `buf` exists during this write.
        unsafe {
            (*self.buf.get())[head % N] = value;
        }

        // Step 4: Publish the new head counter using Release ordering so
        // consumers can safely read the byte.
        self.head_producer
            .store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Remove and return the next element, or `None` if the buffer is empty.
    ///
    /// Uses a **CAS loop** to safely support multiple concurrent consumers:
    ///
    /// 1. Load the current `tail_consumer` counter.
    /// 2. If `tail_consumer == head_producer` the buffer is empty → return
    ///    `None`.
    /// 3. Speculatively read the byte at `buf[tail_consumer % N]`.
    /// 4. Attempt to atomically advance `tail_consumer` via
    ///    `compare_exchange_weak`.
    ///    - **Success**: no other consumer changed `tail_consumer` between
    ///      step 1 and 4, so our read is valid → return the byte.
    ///    - **Failure**: another consumer already advanced `tail_consumer` (it
    ///      consumed the same slot first) → loop back to step 1 and retry with
    ///      the updated `tail_consumer` value.
    ///
    /// The speculative read in step 3 is safe because the producer only writes
    /// to `buf[head_producer % N]` *before* advancing `head_producer` with
    /// `Release` ordering, and we read `head_producer` with `Acquire` in
    /// step 2.  A slot between `tail_consumer` and `head_producer` is
    /// therefore always initialised and stable.
    pub fn pop(&self) -> Option<u8> {
        loop {
            // Step 1: Load both counters with Acquire ordering.
            let tail = self.tail_consumer.load(Ordering::Acquire);
            let head = self.head_producer.load(Ordering::Acquire);

            // Step 2: If the tail is equal to the head, the buffer is empty.
            if tail == head {
                return None;
            }

            // Step 3: Speculatively read the byte from the slot.
            // SAFETY:
            // - `tail != head` guarantees the slot is initialized by the producer.
            // - `tail % N` is in-bounds because N equals the array length.
            // - No mutable reference to `buf` exists during this read.
            let value = unsafe { (*self.buf.get())[tail % N] };
            let next = tail.wrapping_add(1);

            // Step 4: Attempt to transition tail_consumer from `tail` to `next`
            // atomically.  If another consumer changed tail_consumer in the
            // meantime, the CAS fails and we loop back.  Because the counters
            // are free-running, a slot that has wrapped around the entire
            // buffer has a different counter value and cannot be claimed
            // twice.
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

// SAFETY: All mutable access to `buf` is synchronized via atomic counters —
// the producer writes only to `buf[head_producer % N]` before publishing, and
// consumers read only slots between `tail_consumer` and `head_producer`.
unsafe impl<const N: usize> Sync for RingBuffer<N> {}
// SAFETY: Sending the ring buffer transfers ownership of the atomic state and
// buffer.  Thread safety invariants are upheld by the SPMC protocol and atomic
// counters.
unsafe impl<const N: usize> Send for RingBuffer<N> {}
