//! Driver support library for KAOS user-mode Ring-3 device drivers.
//!
//! Provides safe abstractions around driver syscalls:
//! - [`mmio`] — Memory-Mapped I/O mapping and direct volatile register access
//! - [`irq`]  — Hardware interrupt subscription, waiting, and acknowledgment
//! - [`spawn`] — Driver spawning primitives

#![no_std]
#![allow(dead_code)]

// Pull in kernel ABI types via path import.
#[path = "../../kernel/src/syscall/types.rs"]
mod kernel_types;

#[allow(unused_imports)]
pub(crate) use kernel_types::{decode_result, SyscallId};
pub use kernel_types::{SysError, UserDriverGrants};

mod raw;

pub mod dma;
pub mod irq;
pub mod mmio;
pub mod spawn;
