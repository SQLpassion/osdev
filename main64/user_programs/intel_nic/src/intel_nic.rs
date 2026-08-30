//! Intel Gigabit Ethernet (82577LM / I219-V e1000 family) controller and DMA descriptor rings.

use lib_driver::{dma::DmaBuffer, irq, mmio::Mmio, SysError};
use lib_net::{MacAddress, NicDevice};

/// Device register offsets (BAR 0, 32-bit aligned).
pub const REG_CTRL: usize = 0x0000;
pub const REG_STATUS: usize = 0x0008;
pub const REG_EECD: usize = 0x0010;
pub const REG_EERD: usize = 0x0014;
pub const REG_ICR: usize = 0x00C0;
pub const REG_IMS: usize = 0x00D0;
pub const REG_IMC: usize = 0x00D8;
pub const REG_RCTL: usize = 0x0100;
pub const REG_TCTL: usize = 0x0400;
pub const REG_RDBAL: usize = 0x2800;
pub const REG_RDBAH: usize = 0x2804;
pub const REG_RDLEN: usize = 0x2808;
pub const REG_RDH: usize = 0x2810;
pub const REG_RDT: usize = 0x2818;
pub const REG_TDBAL: usize = 0x3800;
pub const REG_TDBAH: usize = 0x3804;
pub const REG_TDLEN: usize = 0x3808;
pub const REG_TDH: usize = 0x3810;
pub const REG_TDT: usize = 0x3818;
pub const REG_RAL0: usize = 0x5400;
pub const REG_RAH0: usize = 0x5404;

/// Control register bit definitions.
pub const CTRL_SLU: u32 = 1 << 6; // Set Link Up
pub const CTRL_RST: u32 = 1 << 26; // Device Reset

/// Receive Control (RCTL) bit definitions.
pub const RCTL_EN: u32 = 1 << 1; // Receiver Enable
pub const RCTL_SBP: u32 = 1 << 2; // Store Bad Packets
pub const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous Enable
pub const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous Enable
pub const RCTL_LPE: u32 = 1 << 5; // Long Packet Reception Enable
pub const RCTL_BAM: u32 = 1 << 15; // Broadcast Accept Mode
pub const RCTL_BSIZE_2048: u32 = 0 << 16; // Receive Buffer Size 2048 Bytes
pub const RCTL_SECRC: u32 = 1 << 26; // Strip Ethernet CRC

/// Transmit Control (TCTL) bit definitions.
pub const TCTL_EN: u32 = 1 << 1; // Transmitter Enable
pub const TCTL_PSP: u32 = 1 << 3; // Pad Short Packets
pub const TCTL_CT_SHIFT: u32 = 4; // Collision Threshold shift
pub const TCTL_COLD_SHIFT: u32 = 12; // Collision Distance shift

/// Interrupt mask bits.
pub const INT_RXT0: u32 = 1 << 7; // Receiver Timer Interrupt
pub const INT_TXDW: u32 = 1 << 0; // Transmit Descriptor Written Back
pub const INT_LSC: u32 = 1 << 2; // Link Status Change

/// Descriptor ring constants.
pub const NUM_RX_DESCRIPTORS: usize = 32;
pub const NUM_TX_DESCRIPTORS: usize = 16;
pub const RX_BUFFER_SIZE: usize = 2048;
pub const TX_BUFFER_SIZE: usize = 2048;

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
}

impl NicModel {
    /// Returns human-readable model name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::E1000e => "Intel 82577LM Gigabit Network Connection",
            Self::I219V => "Intel Ethernet Connection I219-V",
        }
    }
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

    _tx_descs: DmaBuffer,
    _tx_bufs: DmaBuffer,
    tx_tail: usize,
}

impl IntelNicDevice {
    /// Initializes the Intel Gigabit network controller and descriptor DMA rings.
    pub fn init(model: NicModel, mmio: Mmio, irq: u8) -> Result<Self, SysError> {
        // Step 1: Issue global software reset via CTRL register.
        let mut ctrl = mmio.read32(REG_CTRL);
        mmio.write32(REG_CTRL, ctrl | CTRL_RST);

        // Spin until hardware clears RST bit.
        let mut timeout = 10_000;
        while (mmio.read32(REG_CTRL) & CTRL_RST) != 0 && timeout > 0 {
            timeout -= 1;
        }
        if timeout == 0 {
            return Err(SysError::IoError);
        }

        // Re-read CTRL, set link up (SLU), and disable auto-speed detection overrides.
        ctrl = mmio.read32(REG_CTRL);
        mmio.write32(REG_CTRL, ctrl | CTRL_SLU);

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
        if (mac_bytes == [0; 6] || mac_bytes == [0xFF; 6]) || (rah & (1 << 31)) == 0 {
            for i in 0..3 {
                // Request EEPROM read at word address i (start bit 1, addr in bits 8..15).
                mmio.write32(REG_EERD, 1 | ((i as u32) << 8));
                let mut eerd_timeout = 10_000;
                while (mmio.read32(REG_EERD) & (1 << 4)) == 0 && eerd_timeout > 0 {
                    eerd_timeout -= 1;
                }
                let data = ((mmio.read32(REG_EERD) >> 16) & 0xFFFF) as u16;
                mac_bytes[i * 2] = (data & 0xFF) as u8;
                mac_bytes[i * 2 + 1] = ((data >> 8) & 0xFF) as u8;
            }
        }
        let mac = MacAddress(mac_bytes);

        // Step 3: Clear and initialize Multicast Table Array (MTA) registers (128 entries).
        for i in 0..128 {
            mmio.write32(0x5200 + (i * 4), 0);
        }

        // Step 4: Allocate contiguous physical RX DMA ring and payload buffers.
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

        // Configure RX descriptor ring registers in hardware.
        mmio.write32(REG_RDBAL, rx_descs.pa() as u32);
        mmio.write32(REG_RDBAH, (rx_descs.pa() >> 32) as u32);
        mmio.write32(
            REG_RDLEN,
            (NUM_RX_DESCRIPTORS * core::mem::size_of::<RxDesc>()) as u32,
        );
        mmio.write32(REG_RDH, 0);
        mmio.write32(REG_RDT, (NUM_RX_DESCRIPTORS - 1) as u32);

        // Configure and enable receiver (RCTL).
        let rctl_val = RCTL_EN | RCTL_BAM | RCTL_BSIZE_2048 | RCTL_SECRC;
        mmio.write32(REG_RCTL, rctl_val);

        // Step 5: Allocate contiguous physical TX DMA ring and payload buffers.
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

        // Configure TX descriptor ring registers in hardware.
        mmio.write32(REG_TDBAL, tx_descs.pa() as u32);
        mmio.write32(REG_TDBAH, (tx_descs.pa() >> 32) as u32);
        mmio.write32(
            REG_TDLEN,
            (NUM_TX_DESCRIPTORS * core::mem::size_of::<TxDesc>()) as u32,
        );
        mmio.write32(REG_TDH, 0);
        mmio.write32(REG_TDT, 0);

        // Configure and enable transmitter (TCTL).
        // CT = 0x0F (15 collisions), COLD = 0x40 (64 bytes full duplex).
        let tctl_val =
            TCTL_EN | TCTL_PSP | (0x0Fu32 << TCTL_CT_SHIFT) | (0x40u32 << TCTL_COLD_SHIFT);
        mmio.write32(REG_TCTL, tctl_val);

        // Step 6: Subscribe to hardware IRQ if valid.
        if irq != 0 && irq != 0xFF {
            if let Ok(()) = irq::subscribe(irq) {
                mmio.write32(REG_IMS, INT_RXT0 | INT_TXDW | INT_LSC);
            }
        }

        Ok(Self {
            model,
            mmio,
            irq,
            mac,
            _rx_descs: rx_descs,
            _rx_bufs: rx_bufs,
            rx_tail: 0,
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

        // Step 2: Copy packet bytes into the corresponding TX payload buffer.
        let buf_offset = slot * TX_BUFFER_SIZE;
        let tx_slice = self._tx_bufs.as_mut_slice();
        tx_slice[buf_offset..buf_offset + packet.len()].copy_from_slice(packet);

        // Step 3: Populate TX descriptor (CMD: EOP (0x01) | IFCS (0x02) | RS (0x08)).
        let buf_pa = self._tx_bufs.pa() + buf_offset as u64;
        let tx_len = packet.len().max(60); // Minimum Ethernet frame length

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
                    cmd: 0x01 | 0x02 | 0x08, // EOP | IFCS | RS
                    status: 0,               // Clear DD bit for transmission
                    css: 0,
                    special: 0,
                },
            );
        }

        // Step 4: Advance tail pointer and kick hardware by writing TDT.
        self.tx_tail = (self.tx_tail + 1) % NUM_TX_DESCRIPTORS;
        self.mmio.write32(REG_TDT, self.tx_tail as u32);

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
        if (status & 0x01) == 0 {
            return None;
        }

        // Step 2: Extract packet length and copy from DMA payload slot.
        // SAFETY:
        // - Hardware completed DMA transfer for this slot.
        let length =
            unsafe { core::ptr::read_volatile(&((*rx_desc_ptr.add(slot)).length)) as usize };

        let buf_offset = slot * RX_BUFFER_SIZE;
        let rx_slice = self._rx_bufs.as_slice();

        let to_copy = length.min(out_buf.len()).min(RX_BUFFER_SIZE);
        out_buf[..to_copy].copy_from_slice(&rx_slice[buf_offset..buf_offset + to_copy]);

        // Step 3: Reset descriptor status byte.
        // SAFETY:
        // - Writes 0 to status byte to prepare descriptor for reuse.
        unsafe {
            core::ptr::write_volatile(&mut ((*rx_desc_ptr.add(slot)).status), 0);
        }

        // Step 4: Advance RDT register to return descriptor slot to hardware.
        self.mmio.write32(REG_RDT, slot as u32);
        self.rx_tail = (self.rx_tail + 1) % NUM_RX_DESCRIPTORS;

        if to_copy > 0 {
            Some(to_copy)
        } else {
            None
        }
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
mod tests {
    use super::*;

    #[test]
    fn test_nic_model_names() {
        assert_eq!(
            NicModel::E1000e.name(),
            "Intel 82577LM Gigabit Network Connection"
        );
        assert_eq!(NicModel::I219V.name(), "Intel Ethernet Connection I219-V");
    }

    #[test]
    fn test_descriptor_struct_sizes() {
        assert_eq!(core::mem::size_of::<RxDesc>(), 16);
        assert_eq!(core::mem::size_of::<TxDesc>(), 16);
    }
}
