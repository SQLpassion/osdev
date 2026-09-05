//! Shared PCI/MMIO discovery and background-driver event loop for KAOS NIC
//! drivers, extracted from the near-duplicate logic that used to live in
//! both `drivers/rtl8139/src/main.rs` and `drivers/intel_nic/src/main.rs`.

#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod discovery;
pub mod repl;

pub use discovery::{find_pci_device, map_mmio_bar, select_mmio_bar_index, MmioMapError, PciMatch};
pub use repl::build_status;
#[cfg(target_arch = "x86_64")]
pub use repl::run_background_driver;
