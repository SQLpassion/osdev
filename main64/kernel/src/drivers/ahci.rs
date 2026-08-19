//! AHCI (SATA) Driver
//!
//! Experimental AHCI driver to allow the UEFI boot-path to load files from disk.

use crate::debugln;
use crate::drivers::pci;
use crate::memory::{pmm, vmm};
use crate::scheduler;
use crate::sync::spinlock::SpinLock;
use crate::sync::waitqueue::WaitQueue;
use crate::sync::waitqueue_adapter;
use core::sync::atomic;

/// AHCI Command bits (PxCMD)
const AHCI_PORT_CMD_ST: u32 = 1 << 0;
const AHCI_PORT_CMD_SUD: u32 = 1 << 1;
const AHCI_PORT_CMD_POD: u32 = 1 << 2;
const AHCI_PORT_CMD_FRE: u32 = 1 << 4;
const AHCI_PORT_CMD_FR: u32 = 1 << 14;
const AHCI_PORT_CMD_CR: u32 = 1 << 15;
const AHCI_PORT_CMD_CPD: u32 = 1 << 20;
/// PxCMD.ICC (Interface Communication Control), bits 28..31
const AHCI_PORT_CMD_ICC_MASK: u32 = 0xF << 28;
const AHCI_PORT_CMD_ICC_ACTIVE: u32 = 0x1 << 28;

/// HBA capability bits (CAP)
const HBA_CAP_SSS: u32 = 1 << 27; // Supports Staggered Spin-up

/// PxSSTS.DET — device present and Phy communication established
const HBA_PORT_DET_PRESENT: u32 = 3;

/// PxSCTL.DET — initiate interface reset (COMRESET)
const HBA_PORT_SCTL_DET_COMRESET: u32 = 1;

/// PxTFD status bits
const HBA_PORT_TFD_BSY: u32 = 1 << 7;
const HBA_PORT_TFD_DRQ: u32 = 1 << 3;

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
    pub prdt_entry: [HbaPrdtEntry; 44],
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

static AHCI_LOCK: SpinLock<()> = SpinLock::new(());

/// True while one caller owns the AHCI transfer request slot end-to-end,
/// i.e. from claiming command slot 0 in `do_transfer` through observing its
/// completion.
///
/// This driver only ever uses command slot 0, and (since issue #48)
/// `AHCI_LOCK` itself is held only for brief register snapshots/writes
/// rather than across an entire transfer - so, on this single-core kernel,
/// interrupts (and therefore task preemption) are no longer disabled for
/// the whole duration of a transfer. Without a separate exclusion
/// mechanism, a second task could be scheduled in while a first transfer is
/// still mid-flight and start its own `do_transfer` call, and both would
/// race to (re)program the same slot-0 command header/table/FIS. This flag
/// (with `AHCI_REQUEST_WAITQUEUE` below) restores "only one transfer in
/// flight at a time" without requiring interrupts to stay disabled while a
/// task waits its turn - mirrors `ata.rs`'s `REQUEST_IN_FLIGHT`.
static AHCI_REQUEST_IN_FLIGHT: atomic::AtomicBool = atomic::AtomicBool::new(false);

/// Waiters blocked while another task owns the AHCI transfer request slot.
static AHCI_REQUEST_WAITQUEUE: WaitQueue = WaitQueue::new();

/// RAII token for exclusive ownership of the AHCI transfer request slot.
struct AhciRequestSlotGuard;

impl Drop for AhciRequestSlotGuard {
    fn drop(&mut self) {
        // Step 1: release the in-flight marker so a waiting request can
        // claim the slot.
        AHCI_REQUEST_IN_FLIGHT.store(false, atomic::Ordering::Release);

        // Step 2: wake request waiters outside of any lock so blocked tasks
        // can re-contend for the request slot.
        waitqueue_adapter::wake_all_multi(&AHCI_REQUEST_WAITQUEUE);
    }
}

/// Acquires exclusive ownership of the AHCI transfer request slot.
///
/// Mirrors `ata.rs`'s `acquire_request_slot`: in scheduler context this
/// blocks cooperatively on a wait queue so other tasks keep running while
/// waiting; in early-boot/test contexts (no scheduler yet) it falls back to
/// brief spin-waiting instead, since there is nothing else to yield to.
fn acquire_transfer_slot() -> AhciRequestSlotGuard {
    loop {
        // Step 1: fast path - try to claim exclusive ownership.
        if AHCI_REQUEST_IN_FLIGHT
            .compare_exchange(
                false,
                true,
                atomic::Ordering::AcqRel,
                atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            return AhciRequestSlotGuard;
        }

        // Step 2: decide whether cooperative sleeping is possible in this
        // execution context (scheduler running + current task available).
        let maybe_task_id = if scheduler::is_running() {
            scheduler::current_task_id()
        } else {
            None
        };

        if let Some(task_id) = maybe_task_id {
            // Step 3a: scheduler context - sleep while the request slot
            // stays busy. Predicate is rechecked with interrupts disabled by
            // the waitqueue adapter.
            if waitqueue_adapter::sleep_if_multi(&AHCI_REQUEST_WAITQUEUE, task_id, || {
                AHCI_REQUEST_IN_FLIGHT.load(atomic::Ordering::Acquire)
            })
            .should_yield()
            {
                // Hand the CPU to the current owner or another runnable task.
                scheduler::yield_now();
            }
        } else {
            // Step 3b: early boot/test context - no scheduler sleep available.
            core::hint::spin_loop();
        }
    }
}

/// Test-only, non-blocking attempt to claim the AHCI transfer request slot.
///
/// Exposed (not just `pub(crate)`) so integration tests can exercise the
/// mutual-exclusion primitive added by issue #48 directly, mirroring
/// `max_prdt_entries_for`'s rationale below: `do_transfer`'s hardware path
/// cannot be driven end-to-end without a mapped AHCI port, which is not
/// present in every test environment (e.g. default QEMU without
/// `-device ahci`), so this is the closest hardware-independent seam to the
/// "only one transfer in flight at a time" invariant that narrowing
/// `AHCI_LOCK`'s scope now depends on.
pub fn try_acquire_transfer_slot_for_test() -> bool {
    AHCI_REQUEST_IN_FLIGHT
        .compare_exchange(
            false,
            true,
            atomic::Ordering::AcqRel,
            atomic::Ordering::Acquire,
        )
        .is_ok()
}

/// Test-only release counterpart to [`try_acquire_transfer_slot_for_test`].
///
/// Mirrors exactly what `AhciRequestSlotGuard::drop` does, so tests can
/// release a slot claimed via the function above without needing a real
/// `do_transfer` call to construct the (non-`pub`) guard type.
pub fn release_transfer_slot_for_test() {
    AHCI_REQUEST_IN_FLIGHT.store(false, atomic::Ordering::Release);
    waitqueue_adapter::wake_all_multi(&AHCI_REQUEST_WAITQUEUE);
}

/// Initializes the first AHCI controller that exposes an active SATA port.
///
/// A system can have more than one AHCI (class 01:06) controller. On QEMU/Proxmox
/// q35, for example, the chipset's built-in ICH9 AHCI at 00:1f.2 is present but
/// *empty*, while the disk is attached to a separate `ich9-ahci` controller on a
/// PCIe port. Picking the first controller blindly therefore talks to the wrong
/// (empty) HBA — exactly the `det=0 on all ports` failure seen on Proxmox. So we
/// scan every AHCI controller and use the first one that yields a usable device.
pub fn init() {
    let devices = pci::get_devices();

    let mut controller_count = 0usize;
    for dev in devices
        .iter()
        .filter(|d| d.class_code == 0x01 && d.subclass == 0x06)
    {
        controller_count += 1;
        if try_init_controller(dev) {
            return; // Found a controller with an active SATA port.
        }
    }

    crate::console::with_console(|c| {
        if controller_count == 0 {
            let _ = core::fmt::Write::write_fmt(
                c,
                core::format_args!("AHCI init: No controller (Class 01:06) found.\n"),
            );
        } else {
            let _ = core::fmt::Write::write_fmt(
                c,
                core::format_args!(
                    "AHCI init: scanned {} controller(s), none had an active SATA port.\n",
                    controller_count
                ),
            );
        }
    });
    debugln!(
        "AHCI: init found no usable controller ({} scanned).",
        controller_count
    );
}

/// Sets up one AHCI controller and tries to activate a SATA port on it.
///
/// Returns `true` if an active SATA device was found and its command engine was
/// started, `false` otherwise (e.g. an empty controller, so the caller moves on
/// to the next one).
fn try_init_controller(dev: &pci::PciDevice) -> bool {
    // Enable Memory Space (bit 1) and Bus Master (bit 2) in the PCI Command
    // Register. Real hardware may not have these enabled by default, which would
    // make MMIO accesses fail.
    unsafe {
        let mut cmd_status = pci::pci_config_read(dev.bus, dev.device, dev.function, 0x04);
        cmd_status |= (1 << 1) | (1 << 2);
        pci::pci_config_write(dev.bus, dev.device, dev.function, 0x04, cmd_status);
    }

    // BAR5 contains the ABAR (AHCI Base Address Register).
    let bar5 = dev.bars[5];
    let phys_base = match bar5.bar_type {
        pci::BarType::Memory32 { address, .. } => address as u64,
        pci::BarType::Memory64 { address, .. } => address,
        _ => {
            debugln!("AHCI: BAR5 is not a memory BAR (bus {}).", dev.bus);
            return false;
        }
    };

    if phys_base == 0 {
        debugln!("AHCI: Invalid ABAR address (0x0) on bus {}.", dev.bus);
        return false;
    }

    // Identity-map the ABAR MMIO region. The registers for 32 ports fit well
    // within two 4KB pages.
    const ABAR_MAPPED_SIZE: u64 = 2 * 4096;
    let virt_base = phys_base;
    for i in 0..2 {
        let page_addr = virt_base + (i * 4096);
        if !vmm::is_va_mapped(page_addr) {
            vmm::map_virtual_to_physical_uc(page_addr, page_addr);
        }
    }

    // `is_va_mapped` above only answers "is there *a* mapping", not "is it uncacheable",
    // so a pre-existing mapping has to be re-typed explicitly (#63 B4). This matters since
    // the kernel-owned page table maps `EfiReservedMemoryType` / `EFI_MEMORY_RUNTIME`
    // regions write-back — design doc §4 prescribes no cacheability for those — so a
    // firmware that describes this device aperture as Reserved rather than
    // `EfiMemoryMappedIO` would leave the ABAR *cached*, and MMIO register writes could be
    // held in a cache line instead of reaching the HBA. Cheap and idempotent: on the common
    // path the loop above just created the mapping uncacheable and this re-stamps the same
    // bits. Note this is deliberately NOT done for the per-port DMA frames below: those are
    // ordinary PMM RAM sitting inside a 2 MiB write-back huge page shared with unrelated
    // frames, AHCI DMA is cache-coherent on x86, and re-typing that huge page would make
    // 2 MiB of normal RAM uncacheable.
    vmm::configure_uc_mapping(virt_base, ABAR_MAPPED_SIZE);

    let hba_mem = virt_base as *mut HbaMem;

    // SAFETY:
    // - `virt_base` is mapped to the valid physical MMIO region reported by BAR5.
    // - We just mapped the pages ensuring they are present.
    // - Memory accesses are within the bounds of HbaMem.
    unsafe {
        // Enable AHCI mode by setting GHC.AE (bit 31).
        let ghc = core::ptr::read_volatile(&(*hba_mem).ghc);
        core::ptr::write_volatile(&mut (*hba_mem).ghc, ghc | (1 << 31));

        init_ports(hba_mem, phys_base)
    }
}

/// Iterates over implemented ports and initializes the first valid SATA drive.
///
/// Returns `true` if a SATA device was found and started on this controller.
unsafe fn init_ports(hba: *mut HbaMem, phys_base: u64) -> bool {
    // Staggered Spin-up support decides whether we must set PxCMD.SUD to power a
    // device up; on controllers without it, SUD is reserved and must stay clear.
    let cap = core::ptr::read_volatile(&(*hba).cap);
    let sss = (cap & HBA_CAP_SSS) != 0;

    let pi = core::ptr::read_volatile(&(*hba).pi);

    // Step 1: Iterate over implemented ports
    for i in 0..32 {
        if (pi & (1 << i)) == 0 {
            continue;
        }

        let port = &mut (*hba).ports[i];

        // Step 2: Cheap check to skip empty ports
        // spin-up to wake one is empty — skip it without allocating structures or
        // paying the COMRESET/link-training wait. This is every port on the empty
        // built-in ICH9 HBA, so without this the driver spends seconds per port
        // there before reaching the controller that actually has the disk.
        let det = core::ptr::read_volatile(&port.ssts) & 0x0F;
        if det == 0 && !sss {
            continue;
        }

        // Set up our command list / FIS / command table structures and enable
        // FIS reception, so a signature FIS produced by the link bring-up below
        // is latched into PxSIG.
        port_rebase(port);

        // Only reset the link if it is not already established. QEMU and firmware
        // that already used the disk present DET=3 — resetting such a live link
        // is needless and risks a too-short retrain wait on real hardware. A port
        // handed over offline (DET=4) or without communication (DET=1) — common
        // on real hardware — gets a COMRESET / spin-up to bring the Phy online.
        // Step 4: Bring up the port link if not already established
        if det != HBA_PORT_DET_PRESENT && !port_bring_up(port, sss) {
            continue;
        }

        // Step 5: Confirm ATA disk signature after waiting for ready state
        if !port_wait_ready(port) {
            continue;
        }
        if core::ptr::read_volatile(&port.sig) != SATA_SIG_ATA {
            continue;
        }

        // Step 6: Start command engine and mark port active
        port_start(port);
        ACTIVE_PORT = Some(port as *mut HbaPort);
        return true;
    }

    // No active ATA port on this controller. Print diagnostics; the caller will
    // try the next AHCI controller (if any).
    crate::console::with_console(|c| {
        let _ = core::fmt::Write::write_fmt(
            c,
            core::format_args!(
                "AHCI: no active SATA port on controller ABAR={:#x} (PI={:#x}).\n",
                phys_base,
                pi
            ),
        );
        let mut printed = 0;
        for i in 0..32 {
            if (pi & (1 << i)) != 0 {
                let port = &mut (*hba).ports[i];
                let ssts = port.ssts;
                let ipm = (ssts >> 8) & 0x0F;
                let det = ssts & 0x0F;
                let _ = core::fmt::Write::write_fmt(
                    c,
                    core::format_args!(
                        " -> Port {}: SSTS={:#010x} (det={}, ipm={}), SIG={:#010x}\n",
                        i,
                        ssts,
                        det,
                        ipm,
                        port.sig
                    ),
                );
                printed += 1;
                if printed >= 5 {
                    let _ = core::fmt::Write::write_fmt(
                        c,
                        core::format_args!(" -> ... more ports omitted ...\n"),
                    );
                    break;
                }
            }
        }
    });

    false
}

/// Stops the port, allocates DMA memory, and restarts the engine.
unsafe fn port_rebase(port: *mut HbaPort) {
    let p = &mut *port;

    // Step 3.1: Stop command engine (clear ST and FRE).
    let cmd = core::ptr::read_volatile(&p.cmd);
    core::ptr::write_volatile(&mut p.cmd, cmd & !(AHCI_PORT_CMD_ST | AHCI_PORT_CMD_FRE));

    // Wait until FR and CR are cleared. Bounded, and without `PAUSE` (see
    // `delay_ms`): a tight PAUSE loop storms VM exits under KVM.
    for _ in 0..1_000_000 {
        let cmd = core::ptr::read_volatile(&p.cmd);
        if (cmd & AHCI_PORT_CMD_FR) == 0 && (cmd & AHCI_PORT_CMD_CR) == 0 {
            break;
        }
    }

    // Step 3.2: Allocate physical frame for Port Structures
    // We request 1 frame (4096 bytes) per port.
    let frame = pmm::with_pmm(|mgr| mgr.alloc_frame()).expect("AHCI: out of physical memory");
    let frame_phys = frame.physical_address();

    // Identity map the allocated frame so we can zero it and access it via virtual address
    if !vmm::is_va_mapped(frame_phys) {
        vmm::map_virtual_to_physical_uc(frame_phys, frame_phys);
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

    // We must link the Command Table base address into the first slot of the Command List
    let cmd_header = &mut *(frame_phys as *mut HbaCmdHeader);
    cmd_header.prdtl = 1; // We use 1 PRDT entry
    cmd_header.ctba = ct_phys as u32;
    cmd_header.ctbau = (ct_phys >> 32) as u32;

    // Enable FIS reception now so the device's signature FIS is captured during
    // the link bring-up that follows. The command engine (PxCMD.ST) is started
    // later in `port_start`, only after the link is confirmed up.
    for _ in 0..1_000_000 {
        if (core::ptr::read_volatile(&p.cmd) & AHCI_PORT_CMD_CR) == 0 {
            break;
        }
    }

    let cmd = core::ptr::read_volatile(&p.cmd);
    core::ptr::write_volatile(&mut p.cmd, cmd | AHCI_PORT_CMD_FRE);

    debugln!("AHCI: Port structures allocated, FIS reception enabled.");
}

/// Brings a port's Phy online via a wake + COMRESET sequence.
///
/// Real firmware often hands the controller over with the port either offline
/// (`PxSSTS.DET == 4`) or without established communication (`DET == 1`). The
/// previous code only accepted an already-running port (as QEMU presents it), so
/// it never recovered such ports. The COMRESET issued here is the only way to
/// transition a Phy out of the offline state.
///
/// Returns `true` once the link is established (`DET == 3`); the caller then uses
/// `port_wait_ready` before reading `PxSIG`. Returns `false` if no device
/// responded within the timeout.
unsafe fn port_bring_up(port: *mut HbaPort, sss: bool) -> bool {
    let p = &mut *port;

    // Clear any latched SATA errors before touching the link.
    core::ptr::write_volatile(&mut p.serr, 0xFFFF_FFFF);

    // Wake the device: request spin-up (only meaningful when the controller
    // supports staggered spin-up), power it on if cold-presence is reported, and
    // force the interface into the active power-management state.
    let mut cmd = core::ptr::read_volatile(&p.cmd);
    if sss {
        cmd |= AHCI_PORT_CMD_SUD;
    }
    if (cmd & AHCI_PORT_CMD_CPD) != 0 {
        cmd |= AHCI_PORT_CMD_POD;
    }
    cmd = (cmd & !AHCI_PORT_CMD_ICC_MASK) | AHCI_PORT_CMD_ICC_ACTIVE;
    core::ptr::write_volatile(&mut p.cmd, cmd);

    // COMRESET: drive PxSCTL.DET to 1 for at least 1ms, then release it back to
    // 0. This is the only mechanism that pulls a Phy out of the offline (DET=4)
    // state; it also re-establishes a stalled link (DET=1).
    let sctl = core::ptr::read_volatile(&p.sctl);
    core::ptr::write_volatile(&mut p.sctl, (sctl & !0xF) | HBA_PORT_SCTL_DET_COMRESET);
    delay_ms(2);
    core::ptr::write_volatile(&mut p.sctl, sctl & !0xF);

    // Wait for the Phy to establish communication (DET == 3). A present device
    // walks DET 0 -> 1 (detected) -> 3 (link up); an empty port stays at 0. So we
    // grant the full link-training budget only once a device has been detected,
    // and bail out early on a port that never even reports presence. This keeps a
    // controller's empty ports from each costing the full timeout — the main
    // source of the ~2 s init delay on real hardware where CAP.SSS is set (so
    // empty ports are not fast-skipped before bring-up).
    const PRESENCE_BUDGET_MS: u32 = 50; // empty ports give up after this
    const LINK_BUDGET_MS: u32 = 200; // detected ports get the full training window
    let mut established = false;
    let mut device_seen = false;
    let mut waited = 0u32;
    while waited < LINK_BUDGET_MS {
        let det = core::ptr::read_volatile(&p.ssts) & 0x0F;
        if det == HBA_PORT_DET_PRESENT {
            established = true;
            break;
        }
        if det != 0 {
            device_seen = true; // detected: allow the full link-training window
        }
        if !device_seen && waited >= PRESENCE_BUDGET_MS {
            break; // nothing on this port, don't wait out the full timeout
        }
        delay_ms(1);
        waited += 1;
    }
    if !established {
        // No device responded on this port; nothing more to do here.
        return false;
    }

    // Clear errors produced by the reset itself.
    core::ptr::write_volatile(&mut p.serr, 0xFFFF_FFFF);

    debugln!("AHCI: Port Phy online.");
    true
}

/// Waits for the device on a port to leave the busy / data-request state so that
/// its task file and signature register are valid. Returns `false` on timeout.
unsafe fn port_wait_ready(port: *mut HbaPort) -> bool {
    let p = &mut *port;
    for _ in 0..200 {
        let tfd = core::ptr::read_volatile(&p.tfd);
        if (tfd & (HBA_PORT_TFD_BSY | HBA_PORT_TFD_DRQ)) == 0 {
            return true;
        }
        delay_ms(1);
    }
    false
}

/// Starts the command engine for a port whose link is already up.
unsafe fn port_start(port: *mut HbaPort) {
    let p = &mut *port;

    // Ensure the command list engine is idle before (re)starting it.
    for _ in 0..1_000_000 {
        if (core::ptr::read_volatile(&p.cmd) & AHCI_PORT_CMD_CR) == 0 {
            break;
        }
    }

    let mut cmd = core::ptr::read_volatile(&p.cmd);
    cmd |= AHCI_PORT_CMD_FRE; // keep FIS reception enabled
    cmd |= AHCI_PORT_CMD_ST; // start command processing
    core::ptr::write_volatile(&mut p.cmd, cmd);

    debugln!("AHCI: Command engine started.");
}

/// Crude busy-wait used during port bring-up.
///
/// No reliable timer is wired up this early on the UEFI path. Critically, this
/// must NOT use `core::hint::spin_loop()` (`PAUSE`): under KVM/Proxmox,
/// PAUSE-loop-exiting turns a tight PAUSE loop into a storm of VM exits, which
/// made `init` appear to hang for minutes. A plain arithmetic loop runs at native
/// speed inside the guest. The iteration count is a rough calibration; slight
/// over-waiting is harmless as this runs only a handful of times at init.
fn delay_ms(ms: u32) {
    let mut sink: u32 = 0;
    for i in 0..ms.saturating_mul(300_000) {
        sink = sink.wrapping_add(i);
        // Volatile read keeps the loop from being optimized away — without
        // issuing any PAUSE or MMIO access (so it triggers no VM exits).
        unsafe { core::ptr::read_volatile(&sink) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AhciError {
    NotInitialized,
    PortError,
    Timeout,

    /// `sector_count == 0` was requested.
    ///
    /// Unlike a normal "nothing to do" case, this is rejected rather than
    /// treated as a silent no-op: AHCI task files interpret a zero sector
    /// count as a request for the maximum transfer size for the active
    /// command (e.g. 65536 sectors for 48-bit LBA reads/writes), so silently
    /// continuing would mis-program the device instead of doing nothing.
    InvalidSectorCount,

    /// The transfer does not fit in the fixed-size PRDT (see
    /// [`HbaCmdTbl::prdt_entry`]) even in the worst case of maximal
    /// physical-page fragmentation.
    PrdtOverflow,
}

/// Maximum number of Physical Region Descriptor Table entries available in
/// one command table (see [`HbaCmdTbl::prdt_entry`]).
pub const MAX_PRDT_ENTRIES: usize = 44;

/// Computes the worst-case number of PRDT entries needed to describe a
/// transfer of `total_bytes`, assuming the least favorable starting
/// physical-page alignment.
///
/// The real fragmentation loop in [`do_transfer`] emits one PRDT entry per
/// contiguous physical-page run, so a buffer that starts one byte before a
/// page boundary produces the most entries: a 1-byte first chunk, then one
/// entry per subsequent full 4 KiB page. This helper is pure and
/// hardware-independent (no port I/O, no page-table walk) so callers can use
/// it to reject an oversized request up front.
///
/// Exposed (not just `pub(crate)`) so integration tests can exercise the
/// validation logic directly: `do_transfer`'s hardware path cannot be driven
/// end-to-end without a mapped AHCI port, which is not present in every test
/// environment (e.g. default QEMU without `-device ahci`).
pub const fn max_prdt_entries_for(total_bytes: usize) -> usize {
    if total_bytes == 0 {
        return 0;
    }

    // Worst case: 1 byte in the first (unaligned) page, then a full page
    // per remaining entry.
    const FIRST_CHUNK: usize = 1;
    if total_bytes <= FIRST_CHUNK {
        return 1;
    }

    1 + (total_bytes - FIRST_CHUNK).div_ceil(4096)
}

fn virt_to_phys(va: u64) -> Option<u64> {
    use crate::memory::vmm::page_table::{
        pd_index, pd_table_addr, pdp_index, pdp_table_addr, pml4_index, pt_index, pt_table_addr,
        table_at, table_entry, PML4_TABLE_ADDR,
    };

    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4e = table_entry(pml4, pml4_index(va));
    if !pml4e.present() {
        return None;
    }

    let pdp = table_at(pdp_table_addr(va));
    let pdpe = table_entry(pdp, pdp_index(va));
    if !pdpe.present() {
        return None;
    }
    if pdpe.huge() {
        return Some((pdpe.frame() * 4096) + (va & ((1 << 30) - 1))); // 1 GiB huge page
    }

    let pd = table_at(pd_table_addr(va));
    let pde = table_entry(pd, pd_index(va));
    if !pde.present() {
        return None;
    }
    if pde.huge() {
        return Some((pde.frame() * 4096) + (va & ((1 << 21) - 1))); // 2 MiB huge page
    }

    let pt = table_at(pt_table_addr(va));
    let pte = table_entry(pt, pt_index(va));
    if pte.present() {
        Some((pte.frame() * 4096) + (va & 0xFFF))
    } else {
        None
    }
}

pub fn read_sectors(buffer: &mut [u8], lba: u64, sector_count: u32) -> Result<(), AhciError> {
    // Self-contained guard: reject an ambiguous zero-sector request before
    // touching any hardware state (see `AhciError::InvalidSectorCount`).
    // `block.rs` already guards this for its current callers, but this
    // function must not rely on caller discipline for a future caller.
    if sector_count == 0 {
        return Err(AhciError::InvalidSectorCount);
    }

    do_transfer(buffer.as_mut_ptr(), buffer.len(), lba, sector_count, false)
}

pub fn write_sectors(buffer: &[u8], lba: u64, sector_count: u32) -> Result<(), AhciError> {
    // Self-contained guard: same rationale as `read_sectors` above.
    if sector_count == 0 {
        return Err(AhciError::InvalidSectorCount);
    }

    do_transfer(
        buffer.as_ptr() as *mut u8,
        buffer.len(),
        lba,
        sector_count,
        true,
    )
}

fn do_transfer(
    buffer_ptr: *mut u8,
    buffer_len: usize,
    lba: u64,
    sector_count: u32,
    is_write: bool,
) -> Result<(), AhciError> {
    // Step 1: claim exclusive ownership of the (sole) transfer slot for the
    // lifetime of this call. This driver only ever programs command slot 0,
    // so only one `do_transfer` call may be mid-flight at a time. Unlike
    // `AHCI_LOCK` below, waiting here does not disable interrupts, so a task
    // waiting for a previous transfer to finish stays preemptible instead of
    // stalling the timer tick / other interrupt-driven drivers (issue #48).
    let _request = acquire_transfer_slot();

    // Step 2: snapshot the active port and clear any stale pending
    // interrupt-status bits left over from a previous command, under
    // AHCI_LOCK.
    //
    // SAFETY:
    // - `ACTIVE_PORT` is written exactly once, in `init_ports`, before the
    //   scheduler (and therefore any other caller of `do_transfer`) can run;
    //   after boot it is read-only, so reading it here cannot race a writer.
    // - The `is` clear below only clears status bits and does not touch any
    //   state `_request` above doesn't already serialize against other
    //   in-flight transfers.
    // - AHCI_LOCK serializes this snapshot/write against every other brief
    //   register access taken elsewhere in this function.
    let port = unsafe {
        let _guard = AHCI_LOCK.lock();
        let port = ACTIVE_PORT.ok_or(AhciError::NotInitialized)?;
        core::ptr::write_volatile(&mut (*port).is, 0xFFFF_FFFF); // Clear pending interrupt bits
        port
    };

    let total_bytes = sector_count as usize * 512;
    assert!(
        buffer_len >= total_bytes,
        "Buffer too small for AHCI transfer"
    );

    // Reject transfers that cannot possibly fit in the fixed-size PRDT even
    // in the worst-case fragmentation scenario, before any port I/O is
    // issued. This turns what used to be a mid-transfer panic (see the PRDT
    // fragmentation loop below) into a recoverable error returned to the
    // caller.
    if max_prdt_entries_for(total_bytes) > MAX_PRDT_ENTRIES {
        return Err(AhciError::PrdtOverflow);
    }

    let slot = 0; // Use slot 0

    // Step 3: wait for command slot 0 to become free.
    //
    // Locking invariant: only the register snapshot on each iteration below
    // runs under AHCI_LOCK; the wait *between* snapshots is unlocked (and
    // therefore does not disable interrupts). Before issue #48, AHCI_LOCK
    // was held (and interrupts disabled) across this entire loop - up to
    // 1,000,000 iterations - stalling the timer tick and every other
    // interrupt-driven driver for as long as a stuck slot took to clear.
    // `_request` above still guarantees no other caller can reach the
    // programming step below concurrently, so in practice this loop now
    // exits on its first iteration; it and its timeout are kept unchanged to
    // preserve the exact original observable contract on a genuinely wedged
    // controller.
    //
    // Deliberately no `spin_loop()`/`PAUSE` is added here: under KVM,
    // PAUSE-loop-exiting turns a tight PAUSE loop into a storm of VM exits
    // (see `delay_ms`'s doc comment above), which would stretch this loop's
    // already-large iteration budget unpredictably in wall-clock time - the
    // same hazard that made `port_bring_up`'s bring-up delay appear to hang.
    let mut timeout = 1_000_000;
    loop {
        // SAFETY:
        // - `port` was validated in Step 2 and remains valid for the
        //   lifetime of the process (see that SAFETY note).
        // - AHCI_LOCK, reacquired fresh on each iteration, serializes this
        //   read pair against the brief register accesses in Steps 2, 4 and
        //   5 - only the read itself needs to be atomic with respect to
        //   those, not the multi-iteration wait between reads.
        let (ci, sact) = unsafe {
            let _guard = AHCI_LOCK.lock();
            (
                core::ptr::read_volatile(&(*port).ci),
                core::ptr::read_volatile(&(*port).sact),
            )
        };
        if (ci & (1 << slot)) == 0 && (sact & (1 << slot)) == 0 {
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err(AhciError::Timeout);
        }
    }

    // Step 4: program the command list header, command table, PRDT and FIS,
    // then issue the command. This is a bounded, fixed-length sequence of
    // register/memory writes (not an open-ended busy-poll), so it stays
    // under one continuous AHCI_LOCK hold - this also keeps "claim slot 0 ->
    // issue command" atomic with respect to the brief snapshots taken in
    // Steps 3 and 5.
    {
        let _guard = AHCI_LOCK.lock();

        // SAFETY:
        // - `port` was snapshotted in Step 2 while `ACTIVE_PORT` could not
        //   change (see that SAFETY note) and stays valid for the process
        //   lifetime.
        // - `_request` (Step 1) guarantees no other caller can be
        //   concurrently mid-flight on command slot 0, and Step 3 confirmed
        //   slot 0 is not currently owned by hardware either, so this block
        //   is the sole writer of the slot-0 command header/table/FIS for
        //   as long as it runs.
        // - `_guard` (AHCI_LOCK) additionally serializes this block's
        //   port-register reads/writes (`clb`/`clbu`/`ci`) against the brief
        //   snapshots taken in Steps 3 and 5.
        // - `cmd_header`/`cmd_tbl`/`fis` are derived from `clb`/`ctba`
        //   addresses this driver itself programmed in `port_rebase` and
        //   identity-mapped; they stay within the single allocated 4 KiB
        //   per-port frame allocated there.
        unsafe {
            let p = &mut *port;

            let clb_phys = (core::ptr::read_volatile(&p.clb) as u64)
                | ((core::ptr::read_volatile(&p.clbu) as u64) << 32);
            let cmd_header = &mut *((clb_phys
                + (slot as u64 * core::mem::size_of::<HbaCmdHeader>() as u64))
                as *mut HbaCmdHeader);

            let mut cfl = (core::mem::size_of::<FisRegH2D>() / 4) as u8;
            if is_write {
                cfl |= 1 << 6;
            }
            core::ptr::write_volatile(&mut cmd_header.cfl, cfl);
            core::ptr::write_volatile(&mut cmd_header.flags, 0);

            let ctba_phys = (core::ptr::read_volatile(&cmd_header.ctba) as u64)
                | ((core::ptr::read_volatile(&cmd_header.ctbau) as u64) << 32);
            let cmd_tbl = &mut *(ctba_phys as *mut HbaCmdTbl);
            core::ptr::write_bytes(
                cmd_tbl as *mut HbaCmdTbl as *mut u8,
                0,
                core::mem::size_of::<HbaCmdTbl>(),
            );

            let mut prdtl = 0;
            let mut remaining_bytes = total_bytes;
            let mut current_va = buffer_ptr as u64;

            while remaining_bytes > 0 {
                // Defense-in-depth: the upfront `max_prdt_entries_for` check
                // above should already guarantee this never trips, but
                // actual physical fragmentation is only known here (it
                // depends on the live page table), so this remains a
                // returned error rather than a panic in case that estimate
                // is ever wrong.
                if prdtl >= MAX_PRDT_ENTRIES {
                    return Err(AhciError::PrdtOverflow);
                }
                let phys_addr = virt_to_phys(current_va).expect("AHCI: Unmapped buffer VA");
                let bytes_in_page = 4096 - (phys_addr & 0xFFF);
                let chunk_size = remaining_bytes.min(bytes_in_page as usize);

                core::ptr::write_volatile(&mut cmd_tbl.prdt_entry[prdtl].dba, phys_addr as u32);
                core::ptr::write_volatile(
                    &mut cmd_tbl.prdt_entry[prdtl].dbau,
                    (phys_addr >> 32) as u32,
                );
                core::ptr::write_volatile(
                    &mut cmd_tbl.prdt_entry[prdtl].dbc,
                    (chunk_size as u32) - 1,
                ); // 0-indexed count

                prdtl += 1;
                remaining_bytes -= chunk_size;
                current_va += chunk_size as u64;
            }
            core::ptr::write_volatile(&mut cmd_header.prdtl, prdtl as u16);

            let fis = &mut *(cmd_tbl.cfis.as_mut_ptr() as *mut FisRegH2D);
            core::ptr::write_bytes(
                fis as *mut FisRegH2D as *mut u8,
                0,
                core::mem::size_of::<FisRegH2D>(),
            );

            core::ptr::write_volatile(&mut fis.fis_type, 0x27);
            core::ptr::write_volatile(&mut fis.pmport_c, 1 << 7);
            if is_write {
                core::ptr::write_volatile(&mut fis.command, 0x35);
            } else {
                core::ptr::write_volatile(&mut fis.command, 0x25);
            }

            core::ptr::write_volatile(&mut fis.lba0, lba as u8);
            core::ptr::write_volatile(&mut fis.lba1, (lba >> 8) as u8);
            core::ptr::write_volatile(&mut fis.lba2, (lba >> 16) as u8);
            core::ptr::write_volatile(&mut fis.device, 1 << 6);

            core::ptr::write_volatile(&mut fis.lba3, (lba >> 24) as u8);
            core::ptr::write_volatile(&mut fis.lba4, (lba >> 32) as u8);
            core::ptr::write_volatile(&mut fis.lba5, (lba >> 40) as u8);

            core::ptr::write_volatile(&mut fis.countl, sector_count as u8);
            core::ptr::write_volatile(&mut fis.counth, (sector_count >> 8) as u8);

            atomic::compiler_fence(atomic::Ordering::SeqCst);
            core::ptr::write_volatile(&mut p.ci, 1 << slot);
        }
    }

    // Step 5: wait for the command to complete.
    //
    // Same locking invariant as Step 3 (and the same reason for not adding a
    // `spin_loop()`): only the register snapshot below runs under
    // AHCI_LOCK, not the wait between snapshots. This is the loop issue #48
    // specifically called out (up to 5,000,000 iterations).
    let mut timeout2 = 5_000_000;
    loop {
        // SAFETY: see Step 3's snapshot above; identical justification.
        let (ci, is) = unsafe {
            let _guard = AHCI_LOCK.lock();
            (
                core::ptr::read_volatile(&(*port).ci),
                core::ptr::read_volatile(&(*port).is),
            )
        };
        if (ci & (1 << slot)) == 0 {
            break;
        }
        if (is & (1 << 30)) != 0 {
            return Err(AhciError::PortError);
        }
        timeout2 -= 1;
        if timeout2 == 0 {
            return Err(AhciError::Timeout);
        }
    }

    Ok(())
}
