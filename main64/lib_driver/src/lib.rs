//! Driver support library for KAOS user-mode Ring-3 device drivers.
//!
//! Provides safe abstractions around driver syscalls:
//! - [`mmio`] — Memory-Mapped I/O mapping and direct volatile register access
//! - [`spawn`] — Driver spawning primitives
//! - [`drv`]  — Driver name registration/resolution, packet transport, and
//!   status publishing (DrvRegister/DrvLookup, NetSend/NetRecv,
//!   DrvPublishStatus/DrvQuery)
//! - [`client`] — `NicClient`, a Ring-3 handle wrapping `drv`'s
//!   DrvLookup/NetSend/NetRecv/DrvQuery calls for an app talking to an
//!   already-running background NIC driver

#![no_std]
#![allow(dead_code)]

// Pull in kernel ABI types via path import.
#[path = "../../kernel/src/syscall/types.rs"]
mod kernel_types;

#[allow(unused_imports)]
pub(crate) use kernel_types::{decode_result, SyscallId};
pub use kernel_types::{
    SysError, UserArpEntry, UserDriverGrants, UserDriverInfo, UserDriverStatus, MAX_ARP_ENTRIES,
    USER_DRIVER_NAME_LEN,
};

mod raw;

pub mod client;
pub mod dma;
pub mod drv;
pub mod mmio;
pub mod spawn;
