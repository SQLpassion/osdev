//! KAOS Rust Kernel Library
//!
//! This library crate exposes kernel functionality for use by integration tests.
//! Integration tests import this library to access kernel modules and the
//! test framework infrastructure.

#![no_std]
#![no_main]

extern crate alloc;

pub mod allocator;
pub mod arch;
pub mod drivers;
pub mod io;
pub mod logging;
pub mod memory;
pub mod process;
pub mod scheduler;
pub mod sync;
pub mod syscall;
pub mod testing;
