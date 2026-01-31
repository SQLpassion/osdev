//! KAOS Rust Kernel Library
//!
//! This library crate exposes kernel functionality for use by integration tests.
//! Integration tests import this library to access kernel modules and the
//! test framework infrastructure.

#![no_std]
#![no_main]

pub mod apps;
pub mod arch;
pub mod drivers;
pub mod memory;
pub mod testing;
