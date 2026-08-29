#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![allow(dead_code)]

extern crate alloc;

pub mod net;

use lib_driver::{irq, mmio::Mmio};
use lib_kaos::{pci, print, println, process};

/// RTL8139 Register Offsets
const REG_MAC0: usize = 0x00;
const REG_CHIPCMD: usize = 0x37;
const REG_IMR: usize = 0x3C;
const REG_ISR: usize = 0x3E;
const REG_CONFIG1: usize = 0x52;

/// ChipCmd bits
const CMD_RESET: u8 = 0x10;
const CMD_RX_ENABLE: u8 = 0x08;
const CMD_TX_ENABLE: u8 = 0x04;

/// Interrupt bits
const INT_RX_OK: u16 = 0x0001;
const INT_TX_OK: u16 = 0x0004;

#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("[RTL8139] Starting user-mode network driver (Ring 3)...");

    // Step 1: Discover RTL8139 device via PCI subsystem.
    let dev_count = pci::get_pci_device_count().unwrap_or(0);
    let mut found_device = None;

    for i in 0..dev_count {
        let mut dev = pci::UserPciDevice {
            bus: 0,
            device: 0,
            function: 0,
            class_code: 0,
            subclass: 0,
            prog_if: 0,
            revision_id: 0,
            header_type: 0,
            vendor_id: 0,
            device_id: 0,
            interrupt_line: 0,
            interrupt_pin: 0,
            _padding: [0; 2],
            bars: [pci::UserPciBar {
                bar_type: 0,
                flags: 0,
                address: 0,
                size: 0,
                raw_value: 0,
                _padding: 0,
            }; 6],
        };

        if pci::get_pci_device(i, &mut dev).is_ok()
            && dev.vendor_id == 0x10EC
            && dev.device_id == 0x8139
        {
            found_device = Some(dev);
            break;
        }
    }

    let Some(dev) = found_device else {
        println!("[RTL8139] No Realtek RTL8139 PCI device (0x10EC:0x8139) detected.");
        process::exit();
    };

    println!(
        "[RTL8139] Found PCI device at Bus {:02x}, Device {:02x}, Func {:02x}, IRQ Line {}",
        dev.bus, dev.device, dev.function, dev.interrupt_line
    );

    // Step 2: Locate Memory BAR (or Memory32 / Memory64 BAR).
    let mut mmio_bar = None;
    for bar in &dev.bars {
        // bar_type 2 = Memory32, 3 = Memory64
        if (bar.bar_type == 2 || bar.bar_type == 3) && bar.address != 0 && bar.size != 0 {
            mmio_bar = Some(*bar);
            break;
        }
    }

    // If memory BAR is not found, fallback to BAR1
    let (bar_phys, bar_size) = match mmio_bar {
        Some(b) => (b.address, b.size as usize),
        None => {
            if dev.bars[1].address != 0 {
                (dev.bars[1].address, dev.bars[1].size as usize)
            } else {
                (dev.bars[0].address, dev.bars[0].size as usize)
            }
        }
    };

    println!(
        "[RTL8139] Mapping MMIO BAR physical {:#x} (len {} bytes)...",
        bar_phys, bar_size
    );

    // Step 3: Map physical MMIO registers into user address space using Mmio.
    let mmio = match Mmio::map(bar_phys, bar_size) {
        Ok(m) => m,
        Err(e) => {
            println!("[RTL8139] Failed to map MMIO registers: {:?}", e);
            process::exit();
        }
    };

    // Step 4: Power on chip (turn off power-saving mode).
    mmio.write8(REG_CONFIG1, 0x00);

    // Step 5: Perform software reset.
    mmio.write8(REG_CHIPCMD, CMD_RESET);
    let mut timeout = 10000;
    while (mmio.read8(REG_CHIPCMD) & CMD_RESET) != 0 && timeout > 0 {
        timeout -= 1;
    }

    if timeout == 0 {
        println!("[RTL8139] Software reset timed out!");
        process::exit();
    }

    // Step 6: Read hardware MAC address.
    let mac = [
        mmio.read8(REG_MAC0),
        mmio.read8(REG_MAC0 + 1),
        mmio.read8(REG_MAC0 + 2),
        mmio.read8(REG_MAC0 + 3),
        mmio.read8(REG_MAC0 + 4),
        mmio.read8(REG_MAC0 + 5),
    ];

    print!("[RTL8139] Hardware MAC Address: ");
    for (i, b) in mac.iter().enumerate() {
        if i > 0 {
            print!(":");
        }
        print!("{:02x}", b);
    }
    println!();

    // Step 7: Subscribe to device IRQ if available.
    let irq_num = dev.interrupt_line;
    if irq_num != 0 && irq_num != 0xFF {
        if let Err(e) = irq::subscribe(irq_num) {
            println!(
                "[RTL8139] Warning: could not subscribe to IRQ {}: {:?}",
                irq_num, e
            );
        } else {
            println!("[RTL8139] Subscribed to IRQ {}", irq_num);
            // Enable RX and TX interrupts
            mmio.write16(REG_IMR, INT_RX_OK | INT_TX_OK);
        }
    }

    // Step 8: Enable Receiver and Transmitter.
    mmio.write8(REG_CHIPCMD, CMD_RX_ENABLE | CMD_TX_ENABLE);
    println!("[RTL8139] Network device initialized and ready in Ring 3.");

    process::exit()
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
