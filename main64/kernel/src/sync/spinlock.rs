//! Interrupt-masking mutual exclusion spinlock.
//!
//! This module provides a `SpinLock` implementation designed specifically for
//! kernel-space usage. It coordinates safe mutable access to shared data between
//! multiple CPU cores (symmetric multiprocessing) and local interrupt contexts.
//!
//! ## The Interrupt Deadlock Problem
//!
//! In kernel development, acquiring a standard spinlock without disabling local CPU
//! interrupts can cause a kernel deadlock. If a thread holding the lock is interrupted
//! by an interrupt service routine (ISR) on the same CPU, and that ISR attempts to
//! acquire the same lock, the system will hang forever:
//!
//! ```text
//! Thread Context                      Interrupt Context (ISR)
//!   │                                   │
//!   ├──► Acquire SpinLock ──────────────┼──────┐
//!   │    (Interrupts still enabled)      │      │ (Lock is now HELD)
//!   │                                   │      ▼
//!   ├───────────► [Hardware Interrupt Fires] ──► Attempts to acquire SpinLock
//!   │                                          │ (Spins forever waiting for Thread)
//!   │  ◄───────────────────────────────────────┘
//!   │ (Deadlock! Thread never resumes to release the lock)
//! ```
//!
//! To prevent this deadlock, `SpinLock::lock()` implements local interrupt masking:
//! 1. Disables interrupts on the local CPU before spinning on the lock.
//! 2. Uses atomic compare-and-swap (CAS) to acquire the lock.
//! 3. Returns a `SpinLockGuard` which, when dropped, releases the lock and restores
//!    interrupts to their original state (enabled/disabled).

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::interrupts;

/// A mutual exclusion lock utilizing interrupt-masking and atomic busy-waiting.
///
/// Access to the underlying data is wrapped in an `UnsafeCell` to allow interior
/// mutability. Safety is guaranteed by the lock's atomic invariants.
///
/// ## Memory Ordering and Synchronization
///
/// This structure enforces strict memory barriers using `core::sync::atomic::Ordering`:
///
/// * **Acquire Barrier:** Acquisition of the lock via `compare_exchange` uses
///   `Ordering::Acquire`. This ensures that any data reads or writes inside the
///   protected critical section are not reordered before the lock is held.
/// * **Release Barrier:** Releasing the lock via a write to `locked` uses
///   `Ordering::Release`. This ensures all writes performed in the critical section
///   are committed and visible to other CPUs before the lock is released.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        // Step 1: Initialize the lock state as unlocked (false).
        // Step 2: Wrap the protected data in an UnsafeCell to allow interior mutability.
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        // Step 1: Query the current interrupt state.
        let interrupts_were_enabled = interrupts::are_enabled();

        // Step 2: Disable interrupts on the local CPU to prevent deadlocks if an
        // interrupt handler attempts to acquire the same spinlock.
        interrupts::disable();

        // Step 3: Spin-wait until the lock is successfully acquired.
        // We atomically transition `locked` from `false` to `true`.
        // Acquire ordering ensures subsequent memory accesses are not reordered before the lock.
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Yield the CPU execution unit during the spin loop to optimize power and performance.
            core::hint::spin_loop();
        }

        // Step 4: Construct and return the guard that holds the lock state and previous interrupt status.
        SpinLockGuard {
            lock: self,
            interrupts_were_enabled,
        }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    interrupts_were_enabled: bool,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - The spinlock guarantees exclusive access while the guard lives.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - The spinlock guarantees exclusive access while the guard lives.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // Step 1: Release the lock by setting the atomic `locked` flag to false.
        // Release ordering guarantees that all operations performed within the lock are visible
        // to other threads that acquire the lock after us.
        self.lock.locked.store(false, Ordering::Release);

        // Step 2: Restore the CPU interrupt state.
        // If interrupts were enabled before we acquired the lock, re-enable them now.
        if self.interrupts_were_enabled {
            interrupts::enable();
        }
    }
}

// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - Access to `data` is synchronized via the spinlock.
// - `T: Send` ensures it is safe to transfer ownership across threads/CPUs.
unsafe impl<T: Send> Sync for SpinLock<T> {}
// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - Moving `SpinLock<T>` between threads preserves synchronization guarantees.
// - `T: Send` ensures protected payload can cross thread/CPU boundaries.
unsafe impl<T: Send> Send for SpinLock<T> {}
