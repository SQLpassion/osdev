//! AHCI (SATA) Driver
//!
//! Experimental AHCI driver to allow the UEFI boot-path to load files from disk.

use crate::debugln;
use crate::drivers::pci;
use crate::memory::vmm;

/// Generic Host Control registers (HBA Memory Registers)
#[repr(C)]
pub struct HbaMem {
    /// 0x00 - Host capability
    pub cap: u32,
    /// 0x04 - Global host control
    pub ghc: u32,
    /// 0x08 - Interrupt status
    pub is: u32,
    /// 0x0C - Ports implemented
    pub pi: u32,
    /// 0x10 - Version
    pub vs: u32,
    /// 0x14 - Command completion coalescing control
    pub ccc_ctl: u32,
    /// 0x18 - Command completion coalescing ports
    pub ccc_pts: u32,
    /// 0x1C - Enclosure management location
    pub em_loc: u32,
    /// 0x20 - Enclosure management control
    pub em_ctl: u32,
    /// 0x24 - Host capabilities extended
    pub cap2: u32,
    /// 0x28 - BIOS/OS handoff control and status
    pub bohc: u32,

    /// 0x2C - 0x9F, Reserved
    pub rsv: [u8; 0xA0 - 0x2C],

    /// 0xA0 - 0xFF, Vendor specific registers
    pub vendor: [u8; 0x100 - 0xA0],

    /// 0x100 - 0x10FF, Port control registers
    pub ports: [HbaPort; 32],
}

/// Port Control Registers
#[repr(C)]
pub struct HbaPort {
    /// 0x00, command list base address, 1K-byte aligned
    pub clb: u32,
    /// 0x04, command list base address upper 32 bits
    pub clbu: u32,
    /// 0x08, FIS base address, 256-byte aligned
    pub fb: u32,
    /// 0x0C, FIS base address upper 32 bits
    pub fbu: u32,
    /// 0x10, interrupt status
    pub is: u32,
    /// 0x14, interrupt enable
    pub ie: u32,
    /// 0x18, command and status
    pub cmd: u32,
    /// 0x1C, Reserved
    pub rsv0: u32,
    /// 0x20, task file data
    pub tfd: u32,
    /// 0x24, signature
    pub sig: u32,
    /// 0x28, SATA status (SCR0:SStatus)
    pub ssts: u32,
    /// 0x2C, SATA control (SCR2:SControl)
    pub sctl: u32,
    /// 0x30, SATA error (SCR1:SError)
    pub serr: u32,
    /// 0x34, SATA active (SCR3:SActive)
    pub sact: u32,
    /// 0x38, command issue
    pub ci: u32,
    /// 0x3C, SATA notification (SCR4:SNotification)
    pub sntf: u32,
    /// 0x40, FIS-based switch control
    pub fbs: u32,
    /// 0x44 ~ 0x70, Reserved
    pub rsv1: [u32; 11],
    /// 0x70 ~ 0x7F, vendor specific
    pub vendor: [u32; 4],
}

/// Initializes the AHCI controller if present.
pub fn init() {
    // Step 1: Find AHCI controller (Class 0x01, Subclass 0x06)
    let ahci_device = pci::find_by_class(0x01, 0x06);
    let dev = match ahci_device {
        Some(d) => d,
        None => {
            debugln!("AHCI: No controller found.");
            return;
        }
    };

    debugln!(
        "AHCI: Controller found at PCI {:02x}:{:02x}.{}",
        dev.bus,
        dev.device,
        dev.function
    );

    // BAR5 contains the ABAR (AHCI Base Address Register)
    let bar5 = dev.bars[5];
    let phys_base = match bar5.bar_type {
        pci::BarType::Memory32 { address, .. } => address as u64,
        pci::BarType::Memory64 { address, .. } => address,
        _ => {
            debugln!("AHCI: BAR5 is not a memory BAR.");
            return;
        }
    };

    if phys_base == 0 {
        debugln!("AHCI: Invalid ABAR address (0x0).");
        return;
    }

    debugln!("AHCI: ABAR physical base at 0x{:x}", phys_base);

    // Step 2: Map MMIO and initialize HBA
    // We identity map the ABAR physical address to a virtual address.
    // AHCI registers fit well within a 4KB page (0x1100 bytes for 32 ports, up to 0x1100).
    // Let's map 2 pages (8KB) just to be safe and cover all 32 ports.
    let virt_base = phys_base;
    for i in 0..2 {
        let page_addr = virt_base + (i * 4096);
        if !vmm::is_va_mapped(page_addr) {
            vmm::map_virtual_to_physical(page_addr, page_addr);
        }
    }

    let hba_mem = virt_base as *mut HbaMem;

    // SAFETY:
    // - `virt_base` is mapped to the valid physical MMIO region reported by PCI BAR5.
    // - We just mapped the pages ensuring they are present.
    // - Memory accesses are within the bounds of HbaMem.
    unsafe {
        let ptr = &mut *hba_mem;

        // Enable AHCI mode by setting GHC.AE (bit 31)
        ptr.ghc |= 1 << 31;

        debugln!("AHCI: Global Host Control (GHC) = 0x{:08x}", ptr.ghc);
        debugln!("AHCI: Ports Implemented (PI) = 0x{:08x}", ptr.pi);
        debugln!("AHCI: Host Capabilities (CAP) = 0x{:08x}", ptr.cap);
        debugln!("AHCI: Version (VS) = 0x{:08x}", ptr.vs);
    }

    debugln!("AHCI: Initialization Step 1 & 2 complete.");
}
