//! Intel Gigabit Ethernet (82577LM / I219-V e1000 family) controller and DMA descriptor rings.

use lib_driver::{dma::DmaBuffer, irq, mmio::Mmio, SysError};
use lib_net::{MacAddress, NicDevice};

/// Device register offsets (BAR 0, 32-bit aligned).
pub const REG_CTRL: usize = 0x0000;
pub const REG_STATUS: usize = 0x0008;
pub const REG_EECD: usize = 0x0010;
pub const REG_CTRL_EXT: usize = 0x0018;
pub const REG_EERD: usize = 0x0014;
pub const REG_ICR: usize = 0x00C0;
pub const REG_IMS: usize = 0x00D0;
pub const REG_IMC: usize = 0x00D8;
pub const REG_RCTL: usize = 0x0100;
pub const REG_TCTL: usize = 0x0400;
pub const REG_TIPG: usize = 0x0410;
pub const REG_EXTCNF_CTRL: usize = 0x0F00;
pub const REG_PBA: usize = 0x1000;
pub const REG_PBS: usize = 0x1008;
pub const REG_KABGTXD: usize = 0x3004;
pub const REG_TDFH: usize = 0x3410;
pub const REG_TDFT: usize = 0x3418;
pub const REG_TDFHS: usize = 0x3420;
pub const REG_TDFTS: usize = 0x3428;
pub const REG_TDFPC: usize = 0x3430;
pub const REG_RFCTL: usize = 0x5008;
pub const REG_RDBAL: usize = 0x2800;
pub const REG_RDBAH: usize = 0x2804;
pub const REG_RDLEN: usize = 0x2808;
pub const REG_RDH: usize = 0x2810;
pub const REG_RDT: usize = 0x2818;
pub const REG_RXDCTL: usize = 0x2828;
pub const REG_TDBAL: usize = 0x3800;
pub const REG_TDBAH: usize = 0x3804;
pub const REG_TDLEN: usize = 0x3808;
pub const REG_TDH: usize = 0x3810;
pub const REG_TDT: usize = 0x3818;
pub const REG_TXDCTL: usize = 0x3828;
pub const REG_TARC0: usize = 0x3840;
pub const REG_TXDCTL1: usize = 0x3928;
pub const REG_TARC1: usize = 0x3940;
pub const REG_RAL0: usize = 0x5400;
pub const REG_RAH0: usize = 0x5404;
pub const REG_GCR: usize = 0x5B00;
pub const REG_FWSM: usize = 0x5B54;

/// Control register bit definitions.
pub const CTRL_ASDE: u32 = 1 << 5; // Auto-Speed Detection Enable
pub const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2; // Block new PCIe master requests
pub const CTRL_EXT_DRV_LOAD: u32 = 1 << 28; // Driver Loaded
pub const CTRL_EXT_RO_DIS: u32 = 1 << 17; // Disable PCIe relaxed ordering
pub const CTRL_EXT_ICH_REQUIRED: u32 = 1 << 22; // Required ICH/PCH initialization bit
pub const CTRL_SLU: u32 = 1 << 6; // Set Link Up
pub const CTRL_RST: u32 = 1 << 26; // Device Reset
pub const CTRL_VME: u32 = 1 << 30; // VLAN Mode Enable
pub const CTRL_PHY_RST: u32 = 1 << 31; // PCH LAN-connected PHY reset

/// Device Status and PCH firmware-control bits used during reset hand-off.
pub const STATUS_LU: u32 = 1 << 1;
pub const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;
pub const FWSM_RSPCIPHY: u32 = 1 << 6;
pub const EXTCNF_CTRL_SWFLAG: u32 = 1 << 4;

/// PCIe transaction classes that must use coherent snooped accesses.
pub const GCR_PCIE_NO_SNOOP_ALL: u32 = 0x0000_003F;

/// Receive Control (RCTL) bit definitions.
pub const RCTL_EN: u32 = 1 << 1; // Receiver Enable
pub const RCTL_SBP: u32 = 1 << 2; // Store Bad Packets
pub const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous Enable
pub const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous Enable
pub const RCTL_LPE: u32 = 1 << 5; // Long Packet Reception Enable
pub const RCTL_BAM: u32 = 1 << 15; // Broadcast Accept Mode
pub const RCTL_BSIZE_2048: u32 = 0 << 16; // Receive Buffer Size 2048 Bytes
pub const RCTL_SECRC: u32 = 1 << 26; // Strip Ethernet CRC
pub const RCTL_DTYP_MASK: u32 = 0b11 << 10; // Receive Descriptor Type

/// Receive Filter Control (RFCTL) bit definitions.
pub const RFCTL_EXTEN: u32 = 1 << 15; // Extended Receive Descriptor Enable
pub const RFCTL_NFSW_DIS: u32 = 1 << 6; // Disable NFS write-packet filtering
pub const RFCTL_NFSR_DIS: u32 = 1 << 7; // Disable NFS read-packet filtering

/// Transmit Control (TCTL) bit definitions.
pub const TCTL_EN: u32 = 1 << 1; // Transmitter Enable
pub const TCTL_PSP: u32 = 1 << 3; // Pad Short Packets
pub const TCTL_RTLC: u32 = 1 << 24; // Re-transmit on Late Collision
pub const TCTL_MULR: u32 = 1 << 28; // Multiple Request Support
pub const TCTL_CT_SHIFT: u32 = 4; // Collision Threshold shift
pub const TCTL_COLD_SHIFT: u32 = 12; // Collision Distance shift
pub const TCTL_CT_MASK: u32 = 0xFF << TCTL_CT_SHIFT;

/// Interrupt mask bits.
pub const INT_RXT0: u32 = 1 << 7; // Receiver Timer Interrupt
pub const INT_TXDW: u32 = 1 << 0; // Transmit Descriptor Written Back
pub const INT_LSC: u32 = 1 << 2; // Link Status Change

/// Descriptor ring constants.
pub const NUM_RX_DESCRIPTORS: usize = 32;
pub const NUM_TX_DESCRIPTORS: usize = 16;
pub const RX_BUFFER_SIZE: usize = 2048;
pub const TX_BUFFER_SIZE: usize = 2048;

/// Descriptor status/error bits used by the legacy receive format.
pub const RX_STATUS_DD: u8 = 1 << 0;
pub const RX_STATUS_EOP: u8 = 1 << 1;
pub const RX_FRAME_ERROR_MASK: u8 = 0x97;

/// Bit 25 enables queues on newer igb controllers, but is reserved on e1000/e1000e.
pub const IGB_QUEUE_ENABLE: u32 = 1 << 25;

/// Transmit descriptor-control fields used by Intel's e1000/e1000e initialization.
pub const TXDCTL_PTHRESH: u32 = 0x0000_003F;
pub const TXDCTL_WTHRESH: u32 = 0x003F_0000;
pub const TXDCTL_COUNT_DESC: u32 = 1 << 22;
pub const TXDCTL_FULL_TX_DESC_WB: u32 = 0x0101_0000;
pub const TXDCTL_MAX_TX_DESC_PREFETCH: u32 = 0x0100_001F;

/// Required ICH/PCH transmit-arbitration and analog-bias values.
pub const ICH_TARC0_REQUIRED: u32 = (1 << 23) | (1 << 24) | (1 << 26) | (1 << 27);
pub const ICH_TARC1_REQUIRED: u32 = (1 << 24) | (1 << 26) | (1 << 30);
pub const ICH_KABGTXD_BGSQLBIAS: u32 = 0x0005_0000;

/// Receive share of the integrated ICH/PCH packet buffer, in KiB.
///
/// The 82577/I219 packet-buffer allocation survives a software reset. Intel's
/// e1000e initialization therefore programs the split before issuing that reset.
pub const ICH_PCH_PBA_RX_KB: u32 = 26;

/// Conservative TSC delays which remain at least 10/20 ms below a 10 GHz TSC rate.
const PRE_RESET_DELAY_CYCLES: u64 = 100_000_000;
const POST_RESET_DELAY_CYCLES: u64 = 200_000_000;
const RESET_RETRY_DELAY_CYCLES: u64 = 200_000_000;
const TX_COMPLETION_TIMEOUT_CYCLES: u64 = 200_000_000;
const HW_OWNERSHIP_TIMEOUT_CYCLES: u64 = 1_000_000_000;
/// Wait times for PHY link-up on physical hardware can exceed 3 seconds if the
/// link partner has spanning tree (STP) enabled or uses Energy Efficient Ethernet.
/// About five seconds on the approximately 2 GHz TSC assumed by the CLI.
const LINK_UP_TIMEOUT_CYCLES: u64 = 10_000_000_000;

/// 16-byte Legacy RX descriptor format for e1000/e1000e.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

/// 16-byte Legacy TX descriptor format for e1000/e1000e.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

/// Supported Intel network controller hardware model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicModel {
    /// Intel 82577LM Gigabit Network Connection (0x8086:0x10EA).
    E1000e,
    /// Intel Ethernet Connection I219-V (0x8086:0x15B8).
    I219V,
    /// Intel 82574L Gigabit Network Connection (0x8086:0x10D3, QEMU/UTM e1000e).
    E1000e82574L,
    /// Intel 82540EM Gigabit Ethernet Controller (0x8086:0x100E, QEMU/UTM e1000).
    E100082540EM,
}

impl NicModel {
    /// Returns human-readable model name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::E1000e => "Intel 82577LM Gigabit Network Connection",
            Self::I219V => "Intel Ethernet Connection I219-V",
            Self::E1000e82574L => "Intel 82574L Gigabit Network Connection (e1000e)",
            Self::E100082540EM => "Intel 82540EM Gigabit Ethernet Controller (e1000)",
        }
    }

    /// Returns the number of implemented Multicast Table Array registers.
    fn mta_register_count(self) -> usize {
        match self {
            Self::E1000e | Self::I219V => 32,
            Self::E1000e82574L | Self::E100082540EM => 128,
        }
    }

    /// Returns whether the controller is an integrated ICH/PCH MAC.
    fn is_ich_pch(self) -> bool {
        matches!(self, Self::E1000e | Self::I219V)
    }

    /// Returns the RX packet-buffer allocation that must precede a global reset.
    fn packet_buffer_allocation_rx_kb(self) -> Option<u32> {
        if self.is_ich_pch() {
            Some(ICH_PCH_PBA_RX_KB)
        } else {
            None
        }
    }
}

/// Reads the invariant timestamp counter used for short hardware settling delays.
#[cfg(target_arch = "x86_64")]
#[inline]
fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;

    // SAFETY:
    // - `rdtsc` is available in the x86_64 execution mode targeted by KAOS.
    // - The instruction only returns counter state and does not access memory.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }

    ((high as u64) << 32) | low as u64
}

/// Host-test counter for non-x86 builders; production KAOS is always x86_64.
#[cfg(not(target_arch = "x86_64"))]
fn read_tsc() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};

    static TEST_CYCLES: AtomicU64 = AtomicU64::new(0);
    TEST_CYCLES.fetch_add(1, Ordering::Relaxed)
}

/// Waits for at least `cycles` TSC ticks without relying on interrupts or a scheduler.
#[cfg(target_arch = "x86_64")]
fn delay_tsc_cycles(cycles: u64) {
    let start = read_tsc();

    // Device reset is performed before IRQ setup, so a bounded TSC spin is the only
    // timing source guaranteed to be available in this Ring-3 initialization path.
    while read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// Host-test fallback for non-x86 builders; production KAOS is always x86_64.
#[cfg(not(target_arch = "x86_64"))]
fn delay_tsc_cycles(cycles: u64) {
    for _ in 0..cycles.min(1_000_000) {
        core::hint::spin_loop();
    }
}

/// Configures legacy transmit descriptor write-back for one e1000/e1000e model.
#[inline]
fn e1000_tx_descriptor_control(model: NicModel, value: u32) -> u32 {
    let mut configured = (value & !(IGB_QUEUE_ENABLE | TXDCTL_WTHRESH)) | TXDCTL_FULL_TX_DESC_WB;
    if model.is_ich_pch() {
        // The integrated MAC needs an explicit non-zero prefetch threshold.
        // With the reset value PTHRESH=0, real 82577 silicon can accept TDT
        // updates while never fetching the first descriptor; emulators ignore it.
        configured =
            (configured & !TXDCTL_PTHRESH) | TXDCTL_COUNT_DESC | TXDCTL_MAX_TX_DESC_PREFETCH;
    }
    configured
}

/// Enables transmit without clobbering model-specific and reserved TCTL bits.
///
/// In particular, the 82577 defines bit 28 as reserved with a reset value of one
/// and uses bits 29:30 for RRTHRESH.  Building TCTL from zero clears both fields
/// and differs from Intel's e1000e initialization sequence.
#[inline]
fn e1000_transmit_control(value: u32) -> u32 {
    (value & !TCTL_CT_MASK) | TCTL_EN | TCTL_PSP | TCTL_RTLC | (0x0Fu32 << TCTL_CT_SHIFT)
}

/// Returns the family-specific inter-packet-gap value recommended by Intel.
#[inline]
fn e1000_transmit_ipg(model: NicModel) -> u32 {
    if model.is_ich_pch() {
        // 82577 / PCH: IPGT=8, IPGR1=8, IPGR2=7.
        0x0070_2008
    } else {
        // Standalone e1000/e1000e controllers use the legacy copper setting.
        0x0060_200A
    }
}

/// Builds the reset command for one controller family and firmware state.
#[inline]
fn reset_control(model: NicModel, ctrl: u32, fwsm: u32) -> u32 {
    let mut reset = ctrl | CTRL_RST;
    if model.is_ich_pch() && (fwsm & FWSM_RSPCIPHY) != 0 {
        // Intel requires the MAC and connected PHY to be reset together. Firmware
        // gates PHY reset through FWSM.RSPCIPHY when AMT owns that resource.
        reset |= CTRL_PHY_RST;
    }
    reset
}

/// Acquires the integrated MAC/firmware semaphore for the reset transaction.
fn acquire_ich_pch_swflag(mmio: &Mmio) -> bool {
    let start = read_tsc();

    loop {
        // Firmware can briefly own the shared MAC/PHY resource. Only claim the
        // flag after observing it clear, then verify the write was accepted.
        let extcnf = mmio.read32(REG_EXTCNF_CTRL);
        if (extcnf & EXTCNF_CTRL_SWFLAG) == 0 {
            mmio.write32(REG_EXTCNF_CTRL, extcnf | EXTCNF_CTRL_SWFLAG);
            let _ = mmio.read32(REG_STATUS);
            if (mmio.read32(REG_EXTCNF_CTRL) & EXTCNF_CTRL_SWFLAG) != 0 {
                return true;
            }
        }

        if read_tsc().wrapping_sub(start) >= HW_OWNERSHIP_TIMEOUT_CYCLES {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Releases the integrated MAC/firmware semaphore if reset did not clear it.
fn release_ich_pch_swflag(mmio: &Mmio) {
    let extcnf = mmio.read32(REG_EXTCNF_CTRL);
    if (extcnf & EXTCNF_CTRL_SWFLAG) != 0 {
        mmio.write32(REG_EXTCNF_CTRL, extcnf & !EXTCNF_CTRL_SWFLAG);
        let _ = mmio.read32(REG_STATUS);
    }
}

/// Computes the required queue-zero ICH/PCH transmit-arbitration value.
#[inline]
fn ich_tarc0(value: u32) -> u32 {
    value | ICH_TARC0_REQUIRED
}

/// Computes the required queue-one ICH/PCH transmit-arbitration value.
#[inline]
fn ich_tarc1(value: u32, tctl: u32) -> u32 {
    let configured = value | ICH_TARC1_REQUIRED;
    if (tctl & TCTL_MULR) != 0 {
        configured & !(1 << 28)
    } else {
        configured | (1 << 28)
    }
}

/// Copies and zero-pads one Ethernet frame into a hardware-owned TX slot.
fn prepare_tx_frame(packet: &[u8], tx_slot: &mut [u8]) -> Option<usize> {
    if packet.is_empty() || packet.len() > tx_slot.len() {
        return None;
    }

    let tx_len = packet.len().max(60);
    if tx_len > tx_slot.len() {
        return None;
    }

    // Hardware transmits exactly the descriptor length, so clear padding bytes before
    // copying the frame to avoid leaking stale bytes from a previous descriptor use.
    tx_slot[..tx_len].fill(0);
    tx_slot[..packet.len()].copy_from_slice(packet);

    Some(tx_len)
}

/// Validates one completed legacy RX descriptor before exposing its DMA payload.
fn received_frame_len(status: u8, errors: u8, length: usize, capacity: usize) -> Option<usize> {
    if (status & (RX_STATUS_DD | RX_STATUS_EOP)) != (RX_STATUS_DD | RX_STATUS_EOP) {
        return None;
    }
    if (errors & RX_FRAME_ERROR_MASK) != 0 || length == 0 || length > RX_BUFFER_SIZE {
        return None;
    }
    if length > capacity {
        return None;
    }

    Some(length)
}

/// Decides how a polled RX descriptor should be reported, accounting for
/// frames that span more than one descriptor (no EOP on the first one).
///
/// `RxDesc.length` is only the byte count DMA'd into *this* descriptor's
/// buffer, not the full frame length. This driver cannot reassemble a
/// multi-descriptor frame, so every descriptor belonging to one must be
/// dropped — including the later, individually well-formed EOP-bearing
/// descriptor that closes it out, which must never be mistaken for an
/// independent, truncated frame of its own.
///
/// Returns `(frame_len, still_mid_frame)`: `frame_len` is `None` whenever the
/// descriptor is part of a frame this driver is (still) discarding, and
/// `still_mid_frame` is the caller's new `mid_frame_drop` state.
fn received_frame_len_multi(
    was_mid_frame: bool,
    status: u8,
    errors: u8,
    length: usize,
    capacity: usize,
) -> (Option<usize>, bool) {
    let still_mid_frame = (status & RX_STATUS_EOP) == 0;
    let frame_len = if was_mid_frame {
        None
    } else {
        received_frame_len(status, errors, length, capacity)
    };
    (frame_len, still_mid_frame)
}

/// Returns whether the MAC reports an established physical link.
#[inline]
fn status_has_link(status: u32) -> bool {
    (status & STATUS_LU) != 0
}

/// Intel 82577LM / I219-V PCIe Gigabit Ethernet Hardware Device Driver.
pub struct IntelNicDevice {
    model: NicModel,
    mmio: Mmio,
    irq: u8,
    mac: MacAddress,

    _rx_descs: DmaBuffer,
    _rx_bufs: DmaBuffer,
    rx_tail: usize,
    /// Set while recycling the continuation descriptors of a frame that spans
    /// more than one RX descriptor (no EOP on its first descriptor). This
    /// driver cannot reassemble such a frame, so every descriptor belonging
    /// to it must be dropped — including the later, individually well-formed
    /// EOP-bearing descriptor that closes it out, which must NOT be mistaken
    /// for an independent, truncated frame of its own.
    mid_frame_drop: bool,

    _tx_descs: DmaBuffer,
    _tx_bufs: DmaBuffer,
    tx_tail: usize,
}

impl IntelNicDevice {
    /// Waits for link autonegotiation before handing the first frame to TX DMA.
    ///
    /// Linux keeps its network queue stopped until link-up.  KAOS has no background
    /// link-state worker, so the equivalent synchronization belongs in transmit().
    fn wait_for_link(&self) -> bool {
        if status_has_link(self.mmio.read32(REG_STATUS)) {
            return true;
        }

        let start = read_tsc();
        while read_tsc().wrapping_sub(start) < LINK_UP_TIMEOUT_CYCLES {
            if status_has_link(self.mmio.read32(REG_STATUS)) {
                // Flush the final status read before programming the descriptor tail.
                let _ = self.mmio.read32(REG_STATUS);
                return true;
            }
            // Step 1: Delay ~1ms between checks. Polling the PCH too aggressively
            // can starve the internal ME/PHY interconnect and prevent link-up on I219-V.
            delay_tsc_cycles(2_000_000);
        }

        false
    }

    /// Initializes the Intel Gigabit network controller and descriptor DMA rings.
    pub fn init(model: NicModel, mmio: Mmio, irq: u8) -> Result<Self, SysError> {
        lib_kaos::println!("[Intel NIC] Phase 1: Controller reset...");
        // Step 1: Block new PCH PCIe master requests and wait until every request
        // inherited from firmware has retired. Resetting with an outstanding TLP
        // can leave the integrated MAC unable to start a later descriptor read.
        if model.is_ich_pch() {
            let ctrl = mmio.read32(REG_CTRL) | CTRL_GIO_MASTER_DISABLE;
            mmio.write32(REG_CTRL, ctrl);
            let _ = mmio.read32(REG_STATUS);

            let start = read_tsc();
            while (mmio.read32(REG_STATUS) & STATUS_GIO_MASTER_ENABLE) != 0 {
                if read_tsc().wrapping_sub(start) >= HW_OWNERSHIP_TIMEOUT_CYCLES {
                    lib_kaos::println!("[Intel NIC] Error: PCIe master requests did not quiesce.");
                    return Err(SysError::IoError);
                }
                core::hint::spin_loop();
            }
        }

        // Step 2: Quiesce DMA units and interrupts. The STATUS read flushes posted
        // PCIe writes, and the settling delay lets the final TLPs retire.
        mmio.write32(REG_IMC, u32::MAX);
        mmio.write32(REG_RCTL, 0);
        mmio.write32(REG_TCTL, TCTL_PSP);

        // PBA is only reset by a power-on reset, not CTRL.RST. A BIOS can leave
        // the integrated MAC with no usable TX share, in which case TDT advances
        // but TDH never fetches a descriptor. Program the Intel e1000e split now;
        // the following global reset is required to reinitialize buffer pointers.
        if let Some(rx_kb) = model.packet_buffer_allocation_rx_kb() {
            mmio.write32(REG_PBA, rx_kb);
        }
        let _ = mmio.read32(REG_STATUS);
        delay_tsc_cycles(PRE_RESET_DELAY_CYCLES);

        // Step 3: Serialize the reset against the Management Engine. Integrated
        // hardware uses this semaphore for shared PHY and selected MAC resources.
        let swflag_acquired = if model.is_ich_pch() {
            if !acquire_ich_pch_swflag(&mmio) {
                lib_kaos::println!("[Intel NIC] Error: firmware ownership timeout.");
                return Err(SysError::IoError);
            }
            true
        } else {
            false
        };

        // Step 4: Issue a simultaneous MAC/PHY reset when firmware permits it.
        // PCH hardware can hang on an immediate read-back, so wait first.
        let ctrl_before_reset = mmio.read32(REG_CTRL);
        let fwsm_before_reset = mmio.read32(REG_FWSM);
        mmio.write32(
            REG_CTRL,
            reset_control(model, ctrl_before_reset, fwsm_before_reset),
        );
        delay_tsc_cycles(POST_RESET_DELAY_CYCLES);
        if swflag_acquired {
            release_ich_pch_swflag(&mmio);
        }

        // Step 5: A real controller must clear RST itself. Continuing while reset is
        // active makes later ring writes disappear when the delayed reset completes.
        if (mmio.read32(REG_CTRL) & CTRL_RST) != 0 {
            delay_tsc_cycles(RESET_RETRY_DELAY_CYCLES);
            if (mmio.read32(REG_CTRL) & CTRL_RST) != 0 {
                lib_kaos::println!("[Intel NIC] Error: controller reset did not complete.");
                return Err(SysError::IoError);
            }
        }

        // Step 6: Clear reset interrupt state, disable VLAN stripping (the software
        // stack expects the tag in-frame), set link up, and enable auto-speed.
        mmio.write32(REG_IMC, u32::MAX);
        let _ = mmio.read32(REG_ICR);
        let ctrl = mmio.read32(REG_CTRL);
        mmio.write32(
            REG_CTRL,
            (ctrl & !(CTRL_RST | CTRL_VME)) | CTRL_SLU | CTRL_ASDE,
        );
        let _ = mmio.read32(REG_STATUS);

        // Set Driver Loaded and disable relaxed ordering. Integrated ICH/PCH MACs
        // additionally require CTRL_EXT bit 22 after every global reset.
        let mut ctrl_ext = mmio.read32(REG_CTRL_EXT);
        ctrl_ext |= CTRL_EXT_DRV_LOAD | CTRL_EXT_RO_DIS;
        if model.is_ich_pch() {
            ctrl_ext |= CTRL_EXT_ICH_REQUIRED;
        }
        mmio.write32(REG_CTRL_EXT, ctrl_ext);
        let _ = mmio.read32(REG_STATUS);

        // Force coherent descriptor and payload DMA. Firmware can leave one of the
        // six no-snoop transaction classes enabled across a BIOS hand-off.
        if model.is_ich_pch() {
            let gcr = mmio.read32(REG_GCR) & !GCR_PCIE_NO_SNOOP_ALL;
            mmio.write32(REG_GCR, gcr);
            let _ = mmio.read32(REG_STATUS);
        }

        // Intel's ICH/PCH initialization sequence marks descriptor counting as
        // available on both Tx queues and programs the MAC arbitration state.
        // These bits are ignored by emulators but required by 82577/I219 silicon.
        if model.is_ich_pch() {
            let txdctl0 = mmio.read32(REG_TXDCTL) | TXDCTL_COUNT_DESC;
            mmio.write32(REG_TXDCTL, txdctl0);
            let txdctl1 = mmio.read32(REG_TXDCTL1) | TXDCTL_COUNT_DESC;
            mmio.write32(REG_TXDCTL1, txdctl1);

            mmio.write32(REG_TARC0, ich_tarc0(mmio.read32(REG_TARC0)));
            mmio.write32(
                REG_TARC1,
                ich_tarc1(mmio.read32(REG_TARC1), mmio.read32(REG_TCTL)),
            );

            let kabgtxd = mmio.read32(REG_KABGTXD) | ICH_KABGTXD_BGSQLBIAS;
            mmio.write32(REG_KABGTXD, kabgtxd);
            let _ = mmio.read32(REG_STATUS);
        }
        delay_tsc_cycles(PRE_RESET_DELAY_CYCLES);

        lib_kaos::println!("[Intel NIC] Phase 2: Reading MAC address...");
        // Step 2: Read hardware MAC address from Receive Address registers (RAL0 / RAH0).
        let ral = mmio.read32(REG_RAL0);
        let rah = mmio.read32(REG_RAH0);
        let mut mac_bytes = [
            (ral & 0xFF) as u8,
            ((ral >> 8) & 0xFF) as u8,
            ((ral >> 16) & 0xFF) as u8,
            ((ral >> 24) & 0xFF) as u8,
            (rah & 0xFF) as u8,
            ((rah >> 8) & 0xFF) as u8,
        ];

        // If RAL/RAH were uninitialized by firmware (all 0 or all FF), fallback to EERD EEPROM read.
        // e1000/e1000e EERD: Bit 0 = START, bits 8..15 = word addr. Done bit is bit 4 (or bit 1 on older chips).
        if (mac_bytes == [0; 6] || mac_bytes == [0xFF; 6]) || (rah & (1 << 31)) == 0 {
            lib_kaos::println!("[Intel NIC] RAL0/RAH0 uninitialized; reading from EEPROM...");
            for i in 0..3 {
                // Request EEPROM read at word address i.
                mmio.write32(REG_EERD, 1 | ((i as u32) << 8));
                let mut eerd_timeout = 100_000;
                while eerd_timeout > 0 {
                    let val = mmio.read32(REG_EERD);
                    // Check bit 4 (e1000e/ICH/PCH) or bit 1 (legacy 82540)
                    if (val & (1 << 4)) != 0 || (val & (1 << 1)) != 0 {
                        let data = ((val >> 16) & 0xFFFF) as u16;
                        mac_bytes[i * 2] = (data & 0xFF) as u8;
                        mac_bytes[i * 2 + 1] = ((data >> 8) & 0xFF) as u8;
                        break;
                    }
                    eerd_timeout -= 1;
                    core::hint::spin_loop();
                }
            }
        }
        let mac = MacAddress(mac_bytes);

        // Step 5: Write back hardware MAC to RAL0/RAH0 with Address Valid (AV) set.
        // Flush each half in order because PCH manageability shares these registers.
        let low_mac = (mac_bytes[0] as u32)
            | ((mac_bytes[1] as u32) << 8)
            | ((mac_bytes[2] as u32) << 16)
            | ((mac_bytes[3] as u32) << 24);
        let high_mac = (mac_bytes[4] as u32) | ((mac_bytes[5] as u32) << 8) | (1u32 << 31); // Bit 31: Address Valid (AV)
        mmio.write32(REG_RAL0, low_mac);
        let _ = mmio.read32(REG_STATUS);
        mmio.write32(REG_RAH0, high_mac);
        let _ = mmio.read32(REG_STATUS);

        lib_kaos::println!("[Intel NIC] Phase 3: Allocating DMA buffers...");

        // Step 6: Clear only the MTA registers implemented by this MAC family. PCH
        // controllers expose 32 entries; standalone e1000 devices expose 128.
        for i in 0..model.mta_register_count() {
            mmio.write32(0x5200 + (i * 4), 0);
        }

        // Step 7: Allocate contiguous physical RX DMA ring and payload buffers.
        // Descriptor ring: 32 descriptors * 16 bytes = 512 bytes (1 page).
        let rx_descs = DmaBuffer::allocate(1)?;
        // Payload buffers: 32 buffers * 2048 bytes = 65536 bytes (16 pages).
        let rx_bufs = DmaBuffer::allocate(16)?;

        // Populate RX descriptors with physical buffer addresses.
        let rx_desc_ptr = rx_descs.va() as *mut RxDesc;
        for i in 0..NUM_RX_DESCRIPTORS {
            let buf_pa = rx_bufs.pa() + (i * RX_BUFFER_SIZE) as u64;
            // SAFETY:
            // - `rx_descs` is an allocated, page-mapped DMA region.
            // - `i < NUM_RX_DESCRIPTORS` (32 * 16 = 512 bytes < 4096 bytes page).
            unsafe {
                core::ptr::write_volatile(
                    rx_desc_ptr.add(i),
                    RxDesc {
                        buffer_addr: buf_pa,
                        length: 0,
                        checksum: 0,
                        status: 0,
                        errors: 0,
                        special: 0,
                    },
                );
            }
        }

        // Descriptor contents must become globally visible before the ring base and
        // tail are handed to the DMA engine.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Explicitly select legacy 16-byte RX descriptors. Firmware or a previous
        // boot stage may leave RFCTL.EXTEN set, whose write-back layout is different
        // even though it has the same size as `RxDesc`.
        let mut rfctl = mmio.read32(REG_RFCTL) & !RFCTL_EXTEN;
        if model.is_ich_pch() {
            // Intel documents these filter disables as a descriptor-corruption
            // workaround for the integrated MAC family.
            rfctl |= RFCTL_NFSW_DIS | RFCTL_NFSR_DIS;
        }
        mmio.write32(REG_RFCTL, rfctl);

        // Configure RX descriptor ring registers in hardware.
        mmio.write32(REG_RDBAL, rx_descs.pa() as u32);
        mmio.write32(REG_RDBAH, (rx_descs.pa() >> 32) as u32);
        mmio.write32(
            REG_RDLEN,
            (NUM_RX_DESCRIPTORS * core::mem::size_of::<RxDesc>()) as u32,
        );
        mmio.write32(REG_RDH, 0);
        mmio.write32(REG_RDT, (NUM_RX_DESCRIPTORS - 1) as u32);

        // e1000/e1000e enables reception through RCTL, not RXDCTL bit 25. Clear
        // that igb-only queue bit while preserving hardware threshold defaults.
        let rxdctl = mmio.read32(REG_RXDCTL) & !IGB_QUEUE_ENABLE;
        mmio.write32(REG_RXDCTL, rxdctl);

        // Configure and enable receiver (RCTL).
        // DTYP=0 matches the legacy descriptor selected above. Unicast packets are
        // accepted through RAR0; promiscuous modes remain disabled.
        let rctl_val = (RCTL_EN | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC) & !RCTL_DTYP_MASK;
        mmio.write32(REG_RCTL, rctl_val);
        let _ = mmio.read32(REG_STATUS);

        // Step 8: Allocate contiguous physical TX DMA ring and payload buffers.
        // Descriptor ring: 16 descriptors * 16 bytes = 256 bytes (1 page).
        let tx_descs = DmaBuffer::allocate(1)?;
        // Payload buffers: 16 buffers * 2048 bytes = 32768 bytes (8 pages).
        let tx_bufs = DmaBuffer::allocate(8)?;

        // Populate TX descriptors with physical buffer addresses and initial DD (Descriptor Done) status.
        let tx_desc_ptr = tx_descs.va() as *mut TxDesc;
        for i in 0..NUM_TX_DESCRIPTORS {
            let buf_pa = tx_bufs.pa() + (i * TX_BUFFER_SIZE) as u64;
            // SAFETY:
            // - `tx_descs` is an allocated, page-mapped DMA region.
            // - `i < NUM_TX_DESCRIPTORS` (16 * 16 = 256 bytes < 4096 bytes page).
            unsafe {
                core::ptr::write_volatile(
                    tx_desc_ptr.add(i),
                    TxDesc {
                        buffer_addr: buf_pa,
                        length: 0,
                        cso: 0,
                        cmd: 0,
                        status: 1, // Bit 0 = DD (Descriptor Done / Software owned)
                        css: 0,
                        special: 0,
                    },
                );
            }
        }

        // Publish the initialized descriptors before making the TX ring visible.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Configure TX descriptor ring registers in hardware.
        mmio.write32(REG_TDBAL, tx_descs.pa() as u32);
        mmio.write32(REG_TDBAH, (tx_descs.pa() >> 32) as u32);
        mmio.write32(
            REG_TDLEN,
            (NUM_TX_DESCRIPTORS * core::mem::size_of::<TxDesc>()) as u32,
        );
        mmio.write32(REG_TDH, 0);
        mmio.write32(REG_TDT, 0);

        // Configure full-descriptor write-back. ICH/PCH additionally requires
        // descriptor counting (bit 22); queue activation itself remains in TCTL.
        let txdctl = e1000_tx_descriptor_control(model, mmio.read32(REG_TXDCTL));
        mmio.write32(REG_TXDCTL, txdctl);
        if model.is_ich_pch() {
            let txdctl1 = e1000_tx_descriptor_control(model, mmio.read32(REG_TXDCTL1));
            mmio.write32(REG_TXDCTL1, txdctl1);
        }

        // Configure and enable the transmitter with a read-modify-write.  This
        // deliberately preserves COLD, RRTHRESH, and model-specific reserved bits
        // restored by the NVM/global reset sequence.
        let tctl_val = e1000_transmit_control(mmio.read32(REG_TCTL));
        mmio.write32(REG_TCTL, tctl_val);

        // Program the model-specific Transmit Inter Packet Gap.
        mmio.write32(REG_TIPG, e1000_transmit_ipg(model));
        let _ = mmio.read32(REG_STATUS);

        // Step 9: Subscribe to hardware IRQ if valid.
        if irq != 0 && irq != 0xFF {
            if let Ok(()) = irq::subscribe(irq) {
                mmio.write32(REG_IMS, INT_RXT0 | INT_TXDW | INT_LSC);
            }
        }

        lib_kaos::println!("[Intel NIC] Phase 4: Initialization complete.");

        Ok(Self {
            model,
            mmio,
            irq,
            mac,
            _rx_descs: rx_descs,
            _rx_bufs: rx_bufs,
            rx_tail: 0,
            mid_frame_drop: false,
            _tx_descs: tx_descs,
            _tx_bufs: tx_bufs,
            tx_tail: 0,
        })
    }

    /// Returns the active NIC model.
    pub fn model(&self) -> NicModel {
        self.model
    }

    /// Returns the hardware MAC address.
    pub fn mac(&self) -> MacAddress {
        self.mac
    }

    /// Transmits a network packet via the current TX descriptor.
    pub fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError> {
        if packet.is_empty() || packet.len() > TX_BUFFER_SIZE {
            return Err(SysError::InvalidArgument);
        }

        // Do not consume a descriptor while copper autonegotiation is still in
        // progress.  A tail update issued with STATUS.LU=0 is not replayed when the
        // link later becomes active, which made the first ARP request disappear.
        if !self.wait_for_link() {
            lib_kaos::println!(
                "[Intel NIC] TX aborted: link did not become ready (STATUS={:#010x}).",
                self.mmio.read32(REG_STATUS)
            );
            return Err(SysError::IoError);
        }

        let slot = self.tx_tail;
        let tx_desc_ptr = self._tx_descs.va() as *mut TxDesc;

        // Step 1: Verify current TX descriptor is free (DD bit 0 is set).
        // SAFETY:
        // - `tx_desc_ptr` points to the mapped TX descriptor ring inside `_tx_descs`.
        // - `slot < NUM_TX_DESCRIPTORS`.
        let status = unsafe { core::ptr::read_volatile(&((*tx_desc_ptr.add(slot)).status)) };
        if (status & 0x01) == 0 {
            // Descriptor is still held by hardware DMA.
            return Err(SysError::IoError);
        }

        // Step 2: Copy and deterministically pad the packet in its TX payload slot.
        let buf_offset = slot * TX_BUFFER_SIZE;
        let tx_slice = self._tx_bufs.as_mut_slice();
        let tx_slot = &mut tx_slice[buf_offset..buf_offset + TX_BUFFER_SIZE];
        let tx_len = prepare_tx_frame(packet, tx_slot).ok_or(SysError::InvalidArgument)?;

        // Step 3: Populate TX descriptor (CMD: EOP (0x01) | IFCS (0x02) | RS (0x08)).
        let buf_pa = self._tx_bufs.pa() + buf_offset as u64;

        // SAFETY:
        // - `tx_desc_ptr.add(slot)` is within valid mapped memory.
        // - Hardware will begin DMA fetch once TDT register is updated below.
        unsafe {
            core::ptr::write_volatile(
                tx_desc_ptr.add(slot),
                TxDesc {
                    buffer_addr: buf_pa,
                    length: tx_len as u16,
                    cso: 0,
                    cmd: 0x01 | 0x02 | 0x08, // EOP (bit 0) | IFCS (bit 1) | RS (bit 3)
                    status: 0,               // Clear DD bit for transmission
                    css: 0,
                    special: 0,
                },
            );
        }

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Step 4: Advance tail pointer and kick hardware by writing TDT.
        self.tx_tail = (self.tx_tail + 1) % NUM_TX_DESCRIPTORS;
        self.mmio.write32(REG_TDT, self.tx_tail as u32);

        // Flush the posted tail write and verify that the real device accepted it.
        // A virtual NIC generally consumes the descriptor before this read; physical
        // PCIe hardware can otherwise hide an unaccepted posted write until timeout.
        if self.mmio.read32(REG_TDT) != self.tx_tail as u32 {
            self.mmio.write32(REG_TDT, self.tx_tail as u32);
            let _ = self.mmio.read32(REG_STATUS);
            if self.mmio.read32(REG_TDT) != self.tx_tail as u32 {
                lib_kaos::println!(
                    "[Intel NIC] TX error: TDT write rejected (TDH={}, TDT={}).",
                    self.mmio.read32(REG_TDH),
                    self.mmio.read32(REG_TDT)
                );
                return Err(SysError::IoError);
            }
        }

        // RS is set in every descriptor, so DD is a bounded hardware acknowledgement
        // that the frame was fetched. This prevents ARP/IP callers from treating a
        // queued-but-stalled descriptor as a successful transmission.
        let completion_start = read_tsc();
        loop {
            // SAFETY:
            // - The descriptor remains allocated for the lifetime of `self`.
            // - Hardware owns it until it writes DD; volatile observes that DMA write-back.
            let completion =
                unsafe { core::ptr::read_volatile(&((*tx_desc_ptr.add(slot)).status)) };
            if (completion & 0x01) != 0 {
                break;
            }
            if read_tsc().wrapping_sub(completion_start) >= TX_COMPLETION_TIMEOUT_CYCLES {
                // SAFETY:
                // - `slot` remains inside the allocated descriptor ring.
                // - Volatile read captures the exact software-visible descriptor for diagnostics.
                let stalled_desc = unsafe { core::ptr::read_volatile(tx_desc_ptr.add(slot)) };
                lib_kaos::println!(
                    "[Intel NIC] TX timeout: slot={}, TDH={}, TDT={}, STATUS={:#010x}, TXDCTL={:#010x}.",
                    slot,
                    self.mmio.read32(REG_TDH),
                    self.mmio.read32(REG_TDT),
                    self.mmio.read32(REG_STATUS),
                    self.mmio.read32(REG_TXDCTL)
                );
                lib_kaos::println!(
                    "[Intel NIC] TX ring: TCTL={:#010x}, TDBA={:#010x}:{:08x}, TDLEN={}.",
                    self.mmio.read32(REG_TCTL),
                    self.mmio.read32(REG_TDBAH),
                    self.mmio.read32(REG_TDBAL),
                    self.mmio.read32(REG_TDLEN)
                );
                lib_kaos::println!(
                    "[Intel NIC] Packet buffers: PBA={:#010x}, PBS={:#010x}, desc PA={:#018x}.",
                    self.mmio.read32(REG_PBA),
                    self.mmio.read32(REG_PBS),
                    self._tx_descs.pa()
                );
                lib_kaos::println!(
                    "[Intel NIC] PCIe/PCH: CTRL={:#010x}, GCR={:#010x}, FWSM={:#010x}, EXTCNF={:#010x}.",
                    self.mmio.read32(REG_CTRL),
                    self.mmio.read32(REG_GCR),
                    self.mmio.read32(REG_FWSM),
                    self.mmio.read32(REG_EXTCNF_CTRL)
                );
                lib_kaos::println!(
                    "[Intel NIC] TX config: CTRL_EXT={:#010x}, TIPG={:#010x}, TARC0={:#010x}, TARC1={:#010x}.",
                    self.mmio.read32(REG_CTRL_EXT),
                    self.mmio.read32(REG_TIPG),
                    self.mmio.read32(REG_TARC0),
                    self.mmio.read32(REG_TARC1)
                );
                lib_kaos::println!(
                    "[Intel NIC] Descriptor: buf={:#018x}, len={}, cmd={:#04x}, status={:#04x}.",
                    stalled_desc.buffer_addr,
                    stalled_desc.length,
                    stalled_desc.cmd,
                    stalled_desc.status
                );
                lib_kaos::println!(
                    "[Intel NIC] TX FIFO: H={:#x}, T={:#x}, HS={:#x}, TS={:#x}, PC={:#x}.",
                    self.mmio.read32(REG_TDFH),
                    self.mmio.read32(REG_TDFT),
                    self.mmio.read32(REG_TDFHS),
                    self.mmio.read32(REG_TDFTS),
                    self.mmio.read32(REG_TDFPC)
                );
                lib_kaos::println!(
                    "[Intel NIC] RX ring: RDH={}, RDT={}, RDBA={:#010x}:{:08x}, RDLEN={}.",
                    self.mmio.read32(REG_RDH),
                    self.mmio.read32(REG_RDT),
                    self.mmio.read32(REG_RDBAH),
                    self.mmio.read32(REG_RDBAL),
                    self.mmio.read32(REG_RDLEN)
                );
                return Err(SysError::IoError);
            }
            core::hint::spin_loop();
        }

        Ok(())
    }

    /// Polls and copies the next received packet from the RX ring into `out_buf`.
    pub fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize> {
        let slot = self.rx_tail;
        let rx_desc_ptr = self._rx_descs.va() as *mut RxDesc;

        // Step 1: Check if descriptor done (DD bit 0) is set by hardware.
        // SAFETY:
        // - `rx_desc_ptr` points to the mapped RX descriptor ring inside `_rx_descs`.
        // - `slot < NUM_RX_DESCRIPTORS`.
        let status = unsafe { core::ptr::read_volatile(&((*rx_desc_ptr.add(slot)).status)) };
        if (status & RX_STATUS_DD) == 0 {
            return None;
        }

        // DD is written after payload DMA. Prevent compiler/CPU reads of descriptor
        // metadata or payload from moving ahead of the ownership observation.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

        // Step 2: Read descriptor metadata and validate EOP, frame errors, and bounds.
        // SAFETY:
        // - Hardware completed DMA transfer for this slot.
        let length =
            unsafe { core::ptr::read_volatile(&((*rx_desc_ptr.add(slot)).length)) as usize };
        // SAFETY:
        // - DD ownership and the acquire fence above make the descriptor write-back visible.
        let errors = unsafe { core::ptr::read_volatile(&((*rx_desc_ptr.add(slot)).errors)) };

        let buf_offset = slot * RX_BUFFER_SIZE;
        let (frame_len, still_mid_frame) =
            received_frame_len_multi(self.mid_frame_drop, status, errors, length, out_buf.len());
        self.mid_frame_drop = still_mid_frame;
        if let Some(frame_len) = frame_len {
            let rx_slice = self._rx_bufs.as_slice();
            out_buf[..frame_len].copy_from_slice(&rx_slice[buf_offset..buf_offset + frame_len]);
        }

        // Step 3: Clear the complete write-back area before returning ownership.
        // SAFETY:
        // - The descriptor belongs to software while DD is set.
        // - `slot` is inside the mapped RX descriptor ring.
        unsafe {
            core::ptr::write_volatile(&mut ((*rx_desc_ptr.add(slot)).length), 0);
            core::ptr::write_volatile(&mut ((*rx_desc_ptr.add(slot)).checksum), 0);
            core::ptr::write_volatile(&mut ((*rx_desc_ptr.add(slot)).status), 0);
            core::ptr::write_volatile(&mut ((*rx_desc_ptr.add(slot)).errors), 0);
            core::ptr::write_volatile(&mut ((*rx_desc_ptr.add(slot)).special), 0);
        }

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Step 4: Advance RDT register to return descriptor slot to hardware.
        self.mmio.write32(REG_RDT, slot as u32);
        self.rx_tail = (self.rx_tail + 1) % NUM_RX_DESCRIPTORS;

        frame_len
    }

    /// Waits for a hardware IRQ event and clears interrupt cause.
    pub fn wait_irq(&mut self, timeout_ms: u32) -> Result<u32, SysError> {
        if self.irq == 0 || self.irq == 0xFF {
            return Ok(0);
        }

        irq::wait(self.irq, timeout_ms)?;
        let icr = self.mmio.read32(REG_ICR); // Read clears ICR on e1000
        let _ = irq::ack(self.irq);

        Ok(icr)
    }

    /// Shuts down the Intel controller (disables TX/RX and masks interrupts).
    pub fn shutdown(&mut self) {
        self.mmio.write32(REG_IMC, 0xFFFFFFFF);
        self.mmio.write32(REG_RCTL, 0);
        self.mmio.write32(REG_TCTL, 0);
    }
}

impl NicDevice for IntelNicDevice {
    fn mac(&self) -> MacAddress {
        self.mac()
    }

    fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError> {
        self.transmit(packet)
    }

    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize> {
        self.poll_next_packet(out_buf)
    }

    fn shutdown(&mut self) {
        self.shutdown();
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/intel_nic.rs"]
mod tests;
