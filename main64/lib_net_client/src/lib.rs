//! Ring-3 client library for talking to a running background NIC driver
//! through the kernel's `DrvLookup`/`NetSend`/`NetRecv`/`DrvQuery` syscalls
//! (`docs/nic_driver_design.md` §4.7).

#![no_std]
#![allow(dead_code)]

mod client;
pub use client::NicClient;
