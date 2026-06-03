//! Synchronization and Task Blocking Primitives.
//!
//! This module provides the core synchronization primitives utilized throughout
//! the KAOS kernel. The tools here range from basic CPU-level spinlocks to
//! scheduler-aware wait queues and lock-free communication rings.
//!

pub mod ringbuffer;
pub mod singlewaitqueue;
pub mod spinlock;
pub mod waitqueue;
pub mod waitqueue_adapter;
