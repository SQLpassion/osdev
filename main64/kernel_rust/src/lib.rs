//! KAOS Rust Kernel Library
//!
//! This library crate exposes kernel functionality for use by integration tests.
//! Integration tests import this library to access kernel modules and the
//! test framework infrastructure.

#![no_std]
#![no_main]

extern crate alloc;

pub mod apps;
pub mod arch;
pub mod allocator;
pub mod drivers;
pub mod logging;
pub mod memory;
pub mod scheduler;
pub mod syscall;
pub mod sync;
pub mod testing;
