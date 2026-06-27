//! AHCI (SATA) Driver
//!
//! Experimental AHCI driver to allow the UEFI boot-path to load files from disk.

use crate::debugln;
use crate::drivers::pci;
use crate::memory::{pmm, vmm};

/// AHCI Command bits
const AHCI_PORT_CMD_ST: u32 = 1 << 0;
const AHCI_PORT_CMD_FRE: u32 = 1 << 4;
const AHCI_PORT_CMD_FR: u32 = 1 << 14;
const AHCI_PORT_CMD_CR: u32 = 1 << 15;

/// SATA Signatures
const SATA_SIG_ATA: u32 = 0x00000101;

/// Command List Header
#[repr(C)]
pub struct HbaCmdHeader {
    /// DW0
    pub cfl: u8, // Command FIS length in DWORDS, 2 ~ 16
    pub flags: u8,  // A, W, P, C flags
    pub prdtl: u16, // Physical region descriptor table length in entries
    /// DW1
    pub prdbc: u32, // Physical region descriptor byte count transferred
    /// DW2, 3
    pub ctba: u32, // Command table descriptor base address
    pub ctbau: u32, // Command table descriptor base address upper 32 bits
    /// DW4 - 7
    pub rsv1: [u32; 4],
}

#[repr(C)]
pub struct HbaCmdTbl {
    pub cfis: [u8; 64], // Command FIS
    pub acmd: [u8; 16], // ATAPI command
    pub rsv: [u8; 48],
    pub prdt_entry: [HbaPrdtEntry; 1],
}

#[repr(C)]
pub struct HbaPrdtEntry {
    pub dba: u32,  // Data base address
    pub dbau: u32, // Data base address upper 32 bits
    pub rsv0: u32,
    pub dbc: u32, // Byte count (bit 0..21), Interrupt on completion (bit 31)
}

#[repr(C)]
pub struct FisRegH2D {
    pub fis_type: u8, // 0x27
    pub pmport_c: u8, // bit 7 is Command, bit 0..3 is PM port
    pub command: u8,
    pub featurel: u8,
    pub lba0: u8,
    pub lba1: u8,
    pub lba2: u8,
    pub device: u8,
    pub lba3: u8,
    pub lba4: u8,
    pub lba5: u8,
    pub featureh: u8,
    pub countl: u8,
    pub counth: u8,
    pub icc: u8,
    pub control: u8,
    pub rsv1: [u8; 4],
}

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

/// Global pointer to the active AHCI port (for later use in read/write)
static mut ACTIVE_PORT: Option<*mut HbaPort> = None;

/// Physical address of the DMA buffer used for sector reads
static mut DMA_BUFFER_PHYS: u64 = 0;

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

        // Step 3: Find and initialize the first active SATA port
        init_ports(hba_mem);
    }

    debugln!("AHCI: Initialization Step 1 & 2 complete.");
}

/// Iterates over implemented ports and initializes the first valid SATA drive.
unsafe fn init_ports(hba: *mut HbaMem) {
    let pi = (*hba).pi;
    for i in 0..32 {
        if (pi & (1 << i)) != 0 {
            let port = &mut (*hba).ports[i];
            let ssts = port.ssts;

            let ipm = (ssts >> 8) & 0x0F;
            let det = ssts & 0x0F;

            // Check if device is present and active (DET=3, IPM=1)
            if det != 3 || ipm != 1 {
                continue;
            }

            if port.sig != SATA_SIG_ATA {
                debugln!(
                    "AHCI: Device at port {} is not a SATA disk (SIG=0x{:08x})",
                    i,
                    port.sig
                );
                continue;
            }

            debugln!("AHCI: Found SATA drive at port {}", i);
            port_rebase(port);
            ACTIVE_PORT = Some(port as *mut HbaPort);
            return;
        }
    }
    debugln!("AHCI: No active SATA drive found.");
}

/// Stops the port, allocates DMA memory, and restarts the engine.
unsafe fn port_rebase(port: *mut HbaPort) {
    let p = &mut *port;

    // Step 3.1: Stop command engine
    p.cmd &= !AHCI_PORT_CMD_ST;
    p.cmd &= !AHCI_PORT_CMD_FRE;

    // Wait until FR and CR are cleared
    loop {
        if (p.cmd & AHCI_PORT_CMD_FR) == 0 && (p.cmd & AHCI_PORT_CMD_CR) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Step 3.2: Allocate physical frame for Port Structures
    // We request 1 frame (4096 bytes) per port.
    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame()).expect("AHCI: out of physical memory");
    let frame_phys = frame.physical_address();

    // Identity map the allocated frame so we can zero it and access it via virtual address
    if !vmm::is_va_mapped(frame_phys) {
        vmm::map_virtual_to_physical(frame_phys, frame_phys);
    }

    // Zero the frame
    core::ptr::write_bytes(frame_phys as *mut u8, 0, 4096);

    // Offset 0: Command List (1024 bytes)
    p.clb = frame_phys as u32;
    p.clbu = (frame_phys >> 32) as u32;

    // Offset 1024: FIS (256 bytes)
    let fis_phys = frame_phys + 1024;
    p.fb = fis_phys as u32;
    p.fbu = (fis_phys >> 32) as u32;

    // Offset 1280: Command Table for slot 0
    let ct_phys = frame_phys + 1280;

    // Offset 1536: DMA Buffer (1 sector = 512 bytes)
    DMA_BUFFER_PHYS = frame_phys + 1536;

    // We must link the Command Table base address into the first slot of the Command List
    let cmd_header = &mut *(frame_phys as *mut HbaCmdHeader);
    cmd_header.prdtl = 1; // We use 1 PRDT entry
    cmd_header.ctba = ct_phys as u32;
    cmd_header.ctbau = (ct_phys >> 32) as u32;

    // Step 3.3: Restart command engine
    // Wait for any pending operations
    loop {
        if (p.cmd & AHCI_PORT_CMD_CR) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    p.cmd |= AHCI_PORT_CMD_FRE;
    p.cmd |= AHCI_PORT_CMD_ST;

    debugln!("AHCI: Port structures allocated and command engine started.");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AhciError {
    NotInitialized,
    PortError,
    Timeout,
}

pub fn read_sectors(buffer: &mut [u8], lba: u32, sector_count: u8) -> Result<(), AhciError> {
    let port = unsafe { ACTIVE_PORT.ok_or(AhciError::NotInitialized)? };
    let dma_buf = unsafe { DMA_BUFFER_PHYS };
    if dma_buf == 0 {
        return Err(AhciError::NotInitialized);
    }

    let total_bytes = sector_count as usize * 512;
    assert!(
        buffer.len() >= total_bytes,
        "Buffer too small for AHCI read"
    );

    for sec in 0..sector_count {
        let current_lba = lba + sec as u32;

        unsafe {
            let p = &mut *port;
            p.is = 0xFFFF_FFFF; // Clear pending interrupt bits

            let slot = 0; // Use slot 0
            let mut timeout = 1_000_000;
            loop {
                let ci = core::ptr::read_volatile(&p.ci);
                let sact = core::ptr::read_volatile(&p.sact);
                if (ci & (1 << slot)) == 0 && (sact & (1 << slot)) == 0 {
                    break;
                }
                timeout -= 1;
                if timeout == 0 {
                    return Err(AhciError::Timeout);
                }
                core::hint::spin_loop();
            }

            let clb_phys = (p.clb as u64) | ((p.clbu as u64) << 32);
            let cmd_header = &mut *((clb_phys
                + (slot as u64 * core::mem::size_of::<HbaCmdHeader>() as u64))
                as *mut HbaCmdHeader);

            cmd_header.cfl = (core::mem::size_of::<FisRegH2D>() / 4) as u8; // 5 DWORDS
            cmd_header.flags = 0; // Read
            cmd_header.prdtl = 1;

            let ctba_phys = (cmd_header.ctba as u64) | ((cmd_header.ctbau as u64) << 32);
            let cmd_tbl = &mut *(ctba_phys as *mut HbaCmdTbl);
            core::ptr::write_bytes(
                cmd_tbl as *mut HbaCmdTbl as *mut u8,
                0,
                core::mem::size_of::<HbaCmdTbl>(),
            );

            // Setup PRDT
            cmd_tbl.prdt_entry[0].dba = dma_buf as u32;
            cmd_tbl.prdt_entry[0].dbau = (dma_buf >> 32) as u32;
            cmd_tbl.prdt_entry[0].dbc = 512 - 1; // Byte count, 0-indexed (511)

            // Setup Command FIS
            let fis = &mut *(cmd_tbl.cfis.as_mut_ptr() as *mut FisRegH2D);
            fis.fis_type = 0x27; // Register H2D
            fis.pmport_c = 1 << 7; // Command
            fis.command = 0x25; // READ DMA EXT

            fis.lba0 = current_lba as u8;
            fis.lba1 = (current_lba >> 8) as u8;
            fis.lba2 = (current_lba >> 16) as u8;
            fis.device = 1 << 6; // LBA mode

            fis.lba3 = (current_lba >> 24) as u8;
            fis.lba4 = 0;
            fis.lba5 = 0;

            fis.countl = 1; // Read 1 sector
            fis.counth = 0;

            // Issue command
            p.ci = 1 << slot;

            // Wait for completion
            let mut timeout2 = 1_000_000;
            loop {
                let ci = core::ptr::read_volatile(&p.ci);
                if (ci & (1 << slot)) == 0 {
                    break; // Completed
                }
                let is = core::ptr::read_volatile(&p.is);
                if (is & (1 << 30)) != 0 {
                    // Task File Error
                    return Err(AhciError::PortError);
                }
                timeout2 -= 1;
                if timeout2 == 0 {
                    return Err(AhciError::Timeout);
                }
                core::hint::spin_loop();
            }

            // Copy from DMA buffer to caller's buffer
            let dest = buffer.as_mut_ptr().add(sec as usize * 512);
            core::ptr::copy_nonoverlapping(dma_buf as *const u8, dest, 512);
        }
    }
    Ok(())
}
