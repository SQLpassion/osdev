//! RTL8139 Realtek Fast Ethernet PCI user-space device driver.
//!
//! Runs in Ring 3 with hardware isolation using `lib_driver` (`Mmio`, `Irq`, `Dma`).
//! Registers itself as "nic:rtl8139" and runs forever as a background
//! process serving `NetSend`/`NetRecv`/`DrvQuery` requests from apps (Phase
//! 2 Step 6) -- PCI discovery, MMIO mapping, and the event loop are shared
//! with the Intel NIC driver via `lib_driver_runtime`.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(dead_code)]

extern crate alloc;

pub mod rtl8139;

#[cfg(not(test))]
use lib_driver_runtime::PciMatch;
#[cfg(not(test))]
use lib_kaos::{process, serial_println};
#[cfg(not(test))]
use lib_net::NetworkStack;
#[cfg(not(test))]
use rtl8139::Rtl8139Device;

#[cfg(not(test))]
const PCI_TABLE: &[PciMatch] = &[PciMatch {
    vendor_id: 0x10EC,
    device_id: 0x8139,
}];

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
    serial_println!("  KAOS RTL8139 Fast Ethernet Driver (Ring 3)");
    serial_println!("==================================================");

    // Step 1: Discover the RTL8139 device via PCI subsystem.
    let Some((dev, _)) = lib_driver_runtime::find_pci_device(PCI_TABLE) else {
        serial_println!("[RTL8139] Error: No Realtek RTL8139 PCI device (0x10EC:0x8139) found.");
        process::exit();
    };
    serial_println!(
        "[RTL8139] Found device: PCI Bus {:02x}:{:02x}.{:x}, IRQ Line {}",
        dev.bus,
        dev.device,
        dev.function,
        dev.interrupt_line
    );

    // Step 2: Map the MMIO BAR (BAR 1 is standard on RTL8139; BAR 0 is the
    // legacy I/O BAR).
    let mmio = match lib_driver_runtime::map_mmio_bar(&dev, Some(1)) {
        Ok(m) => m,
        Err(lib_driver_runtime::MmioMapError::NoUsableBar) => {
            serial_println!(
                "[RTL8139] Error: MMIO BAR address/size is 0; no grantable MMIO window."
            );
            process::exit();
        }
        Err(lib_driver_runtime::MmioMapError::Map(e)) => {
            serial_println!("[RTL8139] Failed to map MMIO registers: {:?}", e);
            process::exit();
        }
    };

    // Step 3: Initialize the RTL8139 controller and DMA ring buffers.
    let device = match Rtl8139Device::init(mmio, dev.interrupt_line) {
        Ok(d) => d,
        Err(e) => {
            serial_println!("[RTL8139] Device initialization failed: {:?}", e);
            process::exit();
        }
    };

    let mac = device.mac();
    serial_println!("[RTL8139] Hardware MAC Address: {}", mac);

    // Step 4: Initialize the protocol network stack.
    let stack = NetworkStack::new(mac);
    serial_println!(
        "[RTL8139] Network initialized: IP {}, Gateway {}",
        stack.config.ip,
        stack.config.gateway
    );

    // Step 5: Hand off to the shared background event loop -- registers as
    // "nic:rtl8139" and never returns under normal operation.
    lib_driver_runtime::run_background_driver(device, stack, "nic:rtl8139")
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
