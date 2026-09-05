//! Intel Gigabit Ethernet (82577LM / I219-V) PCI user-space device driver.
//!
//! Runs in Ring 3 with hardware isolation using `lib_driver` (`Mmio`, `Irq`, `Dma`) and `lib_net`.
//! Registers itself as "nic:intel_nic" and runs forever as a background
//! process serving `NetSend`/`NetRecv`/`DrvQuery` requests from apps (Phase
//! 2 Step 6) -- PCI discovery, MMIO mapping, and the event loop are shared
//! with the RTL8139 driver via `lib_driver_runtime`.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(dead_code)]

extern crate alloc;

pub mod intel_nic;

#[cfg(not(test))]
use intel_nic::{IntelNicDevice, NicModel};
#[cfg(not(test))]
use lib_driver_runtime::PciMatch;
#[cfg(not(test))]
use lib_kaos::{println, process};
#[cfg(not(test))]
use lib_net::NetworkStack;

/// Driver names this table's index into `NicModel` values it was matched
/// against -- see the `models` table alongside this constant.
#[cfg(not(test))]
const PCI_TABLE: &[PciMatch] = &[
    PciMatch {
        vendor_id: 0x8086,
        device_id: 0x10EA,
    },
    PciMatch {
        vendor_id: 0x8086,
        device_id: 0x15B8,
    },
    PciMatch {
        vendor_id: 0x8086,
        device_id: 0x10D3,
    },
    PciMatch {
        vendor_id: 0x8086,
        device_id: 0x100E,
    },
];

/// `NicModel` for each entry in `PCI_TABLE`, at the same index.
#[cfg(not(test))]
const MODELS: &[NicModel] = &[
    NicModel::E1000e,
    NicModel::I219V,
    NicModel::E1000e82574L,
    NicModel::E100082540EM,
];

#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("==================================================");
    println!("  KAOS Intel Gigabit Ethernet Driver (Ring 3)");
    println!("  Supports 82577LM (8086:10EA) & I219-V (8086:15B8)");
    println!("==================================================");

    // Step 1: Discover a supported Intel NIC device via PCI subsystem.
    let Some((dev, table_idx)) = lib_driver_runtime::find_pci_device(PCI_TABLE) else {
        println!("[Intel NIC] Error: No supported Intel Gigabit Ethernet PCI device found.");
        process::exit();
    };
    let model = MODELS[table_idx];
    println!(
        "[Intel NIC] Found {}: PCI Bus {:02x}:{:02x}.{:x}, IRQ Line {}",
        model.name(),
        dev.bus,
        dev.device,
        dev.function,
        dev.interrupt_line
    );

    // Step 2: Map the MMIO BAR (BAR 0 is primary MMIO on Intel NICs).
    let mmio = match lib_driver_runtime::map_mmio_bar(&dev, Some(0)) {
        Ok(m) => m,
        Err(lib_driver_runtime::MmioMapError::NoUsableBar) => {
            println!("[Intel NIC] Error: BAR 0 MMIO address/size is 0; no grantable MMIO window.");
            process::exit();
        }
        Err(lib_driver_runtime::MmioMapError::Map(e)) => {
            println!("[Intel NIC] Failed to map MMIO registers: {:?}", e);
            process::exit();
        }
    };

    println!("[Intel NIC] Initializing hardware controller and DMA rings...");

    // Step 3: Initialize the Intel controller and DMA descriptor rings.
    let device = match IntelNicDevice::init(model, mmio, dev.interrupt_line) {
        Ok(d) => d,
        Err(e) => {
            println!("[Intel NIC] Device initialization failed: {:?}", e);
            process::exit();
        }
    };

    let mac = device.mac();
    println!("[Intel NIC] Hardware MAC Address: {}", mac);

    // Step 4: Initialize the protocol network stack.
    let stack = NetworkStack::new(mac);
    println!(
        "[Intel NIC] Network initialized: IP {}, Gateway {}",
        stack.config.ip, stack.config.gateway
    );

    // Step 5: Hand off to the shared background event loop -- registers as
    // "nic:intel_nic" and never returns under normal operation.
    lib_driver_runtime::run_background_driver(device, stack, "nic:intel_nic")
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
