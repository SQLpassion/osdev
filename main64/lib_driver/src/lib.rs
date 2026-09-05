//! Driver support library for KAOS user-mode Ring-3 device drivers.
//!
//! Provides safe abstractions around driver syscalls:
//! - [`mmio`] — Memory-Mapped I/O mapping and direct volatile register access
//! - [`irq`]  — Hardware interrupt subscription, waiting, and acknowledgment
//! - [`spawn`] — Driver spawning primitives
//! - [`drv`]  — Driver name registration/resolution, packet transport, and
//!   status publishing (DrvRegister/DrvLookup, NetSend/NetRecv,
//!   DrvPublishStatus/DrvQuery)

#![no_std]
#![allow(dead_code)]

// Pull in kernel ABI types via path import.
#[path = "../../kernel/src/syscall/types.rs"]
mod kernel_types;

#[allow(unused_imports)]
pub(crate) use kernel_types::{decode_result, SyscallId};
pub use kernel_types::{
    SysError, UserArpEntry, UserDriverGrants, UserDriverStatus, MAX_ARP_ENTRIES,
};

mod raw;

pub mod dma;
pub mod drv;
pub mod irq;
pub mod mmio;
pub mod spawn;
