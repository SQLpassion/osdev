//! Intel Gigabit Ethernet (82577LM / I219-V) PCI user-space device driver.
//!
//! Runs in Ring 3 with hardware isolation using `lib_driver` (`Mmio`, `Dma`) and `lib_net`.
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
use lib_kaos::{process, serial_println};
#[cfg(not(test))]
use lib_net::NetworkStack;

/// Selects the hardware-quirk `NicModel` for a (vendor_id, device_id) pair.
///
/// This is a distinct concern from PCI discovery: by the time this runs, the
/// kernel has already bound this task to a device from its own validated
/// `driver_db::DRIVER_DB` (see `_start`'s use of `find_bound_device`), so an
/// ID this function does not recognize means this driver's own model table
/// has fallen out of sync with the kernel's — an internal inconsistency
/// worth a specific error, not a normal "device not present" case.
#[cfg(not(test))]
fn model_for_device(vendor_id: u16, device_id: u16) -> Option<NicModel> {
    match (vendor_id, device_id) {
        (0x8086, 0x10EA) => Some(NicModel::E1000e),
        (0x8086, 0x15B8) => Some(NicModel::I219V),
        (0x8086, 0x10D3) => Some(NicModel::E1000e82574L),
        (0x8086, 0x100E) => Some(NicModel::E100082540EM),
        _ => None,
    }
}

#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    // Diagnostic output goes to the serial port, not the VGA console: this
    // is a background process that keeps running after startup, and its
    // own console writes would otherwise scroll away whatever foreground
    // REPL (shell/drivers.bin) happens to be blocked on keyboard input the
    // moment the scheduler switches to this task.
    serial_println!("==================================================");
    serial_println!("  KAOS Intel Gigabit Ethernet Driver (Ring 3)");
    serial_println!("  Supports 82577LM (8086:10EA) & I219-V (8086:15B8)");
    serial_println!("==================================================");

    // Step 1: Recover the Intel NIC device the kernel bound this task to at
    // `SpawnDriver` time (`driver_db::derive_grants`), rather than
    // independently re-scanning the PCI bus.
    let Some(dev) = lib_driver_runtime::find_bound_device() else {
        serial_println!("[Intel NIC] Error: no PCI device bound to this driver task.");
        process::exit();
    };
    let Some(model) = model_for_device(dev.vendor_id, dev.device_id) else {
        serial_println!(
            "[Intel NIC] Error: bound device {:04x}:{:04x} has no known NicModel.",
            dev.vendor_id,
            dev.device_id
        );
        process::exit();
    };
    serial_println!(
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
            serial_println!(
                "[Intel NIC] Error: BAR 0 MMIO address/size is 0; no grantable MMIO window."
            );
            process::exit();
        }
        Err(lib_driver_runtime::MmioMapError::Map(e)) => {
            serial_println!("[Intel NIC] Failed to map MMIO registers: {:?}", e);
            process::exit();
        }
    };

    serial_println!("[Intel NIC] Initializing hardware controller and DMA rings...");

    // Step 3: Initialize the Intel controller and DMA descriptor rings.
    let device = match IntelNicDevice::init(model, mmio) {
        Ok(d) => d,
        Err(e) => {
            serial_println!("[Intel NIC] Device initialization failed: {:?}", e);
            process::exit();
        }
    };

    let mac = device.mac();
    serial_println!("[Intel NIC] Hardware MAC Address: {}", mac);

    // Step 4: Initialize the protocol network stack.
    let stack = NetworkStack::new(mac);
    serial_println!(
        "[Intel NIC] Network initialized: IP {}, Gateway {}",
        stack.config.ip,
        stack.config.gateway
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
