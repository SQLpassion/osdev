//! RTL8139 hardware controller, DMA ring buffers, and interrupt handling.

use lib_driver::{dma::DmaBuffer, irq, mmio::Mmio, SysError};
use lib_net::{MacAddress, NicDevice};

/// RTL8139 Register Offsets
pub const REG_MAC0: usize = 0x00;
pub const REG_MAR0: usize = 0x08;
pub const REG_TSD0: usize = 0x10;
pub const REG_TSAD0: usize = 0x20;
pub const REG_RBSTART: usize = 0x30;
pub const REG_CHIPCMD: usize = 0x37;
pub const REG_CAPR: usize = 0x38;
pub const REG_CBR: usize = 0x3A;
pub const REG_IMR: usize = 0x3C;
pub const REG_ISR: usize = 0x3E;
pub const REG_TCR: usize = 0x40;
pub const REG_RCR: usize = 0x44;
pub const REG_CONFIG1: usize = 0x52;

/// ChipCmd register bits
pub const CMD_BUF_EMPTY: u8 = 0x01;
pub const CMD_TX_ENABLE: u8 = 0x04;
pub const CMD_RX_ENABLE: u8 = 0x08;
pub const CMD_RESET: u8 = 0x10;

/// Interrupt Status / Mask bits
pub const INT_RX_OK: u16 = 0x0001;
pub const INT_RX_ERR: u16 = 0x0002;
pub const INT_TX_OK: u16 = 0x0004;
pub const INT_TX_ERR: u16 = 0x0008;

/// Receive Configuration Register bits
pub const RCR_AAP: u32 = 1 << 0; // Accept All Packets
pub const RCR_APM: u32 = 1 << 1; // Accept Physical Match (destination == MAC)
pub const RCR_AM: u32 = 1 << 2; // Accept Multicast
pub const RCR_AB: u32 = 1 << 3; // Accept Broadcast
pub const RCR_WRAP: u32 = 1 << 7; // Wrap at end of buffer

/// Size of standard RTL8139 RX ring buffer (8K ring + 16-byte header + 2K wrap margin = 4 pages).
pub const RX_RING_PAGES: usize = 4;
pub const RX_RING_SIZE: usize = 8192;

/// Size of TX buffer pool (4 descriptors x 2048 bytes = 2 pages).
pub const TX_POOL_PAGES: usize = 2;
pub const TX_BUFFER_SLOT_SIZE: usize = 2048;

/// Highest physical address the RTL8139 can address for DMA.
///
/// `REG_RBSTART` and `REG_TSAD0..3` are 32-bit registers with no
/// high-address extension (unlike e.g. Intel's `RDBAH`/`TDBAH`), so the
/// chip can only ever DMA to/from physical memory below 4 GiB. Truncating
/// a higher physical address with `as u32` would silently alias it to an
/// unrelated low page instead of failing.
const MAX_DMA_PHYS_ADDR: u64 = u32::MAX as u64;

/// Fails loudly if `[base, base + len)` extends past the RTL8139's 32-bit
/// DMA address space, instead of letting a truncating `as u32` cast alias
/// the buffer to the wrong physical page.
fn require_dma_addressable(base: u64, len: usize) -> Result<(), SysError> {
    let end_inclusive = base.saturating_add(len as u64).saturating_sub(1);
    if end_inclusive > MAX_DMA_PHYS_ADDR {
        return Err(SysError::IoError);
    }
    Ok(())
}

/// RTL8139 Hardware Device Driver.
pub struct Rtl8139Device {
    mmio: Mmio,
    irq: u8,
    mac: MacAddress,
    _rx_buffer: DmaBuffer,
    rx_offset: usize,
    _tx_buffers: DmaBuffer,
    tx_cur: usize,
}

impl Rtl8139Device {
    /// Initializes the RTL8139 hardware controller.
    pub fn init(mmio: Mmio, irq: u8) -> Result<Self, SysError> {
        // Step 1: Power on chip (clear power saving).
        mmio.write8(REG_CONFIG1, 0x00);

        // Step 2: Perform software reset.
        mmio.write8(REG_CHIPCMD, CMD_RESET);
        let mut timeout = 10000;
        while (mmio.read8(REG_CHIPCMD) & CMD_RESET) != 0 && timeout > 0 {
            timeout -= 1;
        }
        if timeout == 0 {
            return Err(SysError::IoError);
        }

        // Step 3: Read hardware MAC address.
        let mac = MacAddress([
            mmio.read8(REG_MAC0),
            mmio.read8(REG_MAC0 + 1),
            mmio.read8(REG_MAC0 + 2),
            mmio.read8(REG_MAC0 + 3),
            mmio.read8(REG_MAC0 + 4),
            mmio.read8(REG_MAC0 + 5),
        ]);

        // Step 4: Allocate contiguous physical RX DMA buffer (16 KiB).
        let rx_buffer = DmaBuffer::allocate(RX_RING_PAGES)?;
        require_dma_addressable(rx_buffer.pa(), RX_RING_PAGES * 4096)?;
        mmio.write32(REG_RBSTART, rx_buffer.pa() as u32);

        // Step 5: Allocate contiguous physical TX DMA buffers (4 x 2048 bytes).
        let tx_buffers = DmaBuffer::allocate(TX_POOL_PAGES)?;
        require_dma_addressable(tx_buffers.pa(), TX_POOL_PAGES * 4096)?;
        for i in 0..4 {
            let slot_pa = tx_buffers.pa() + (i * TX_BUFFER_SLOT_SIZE) as u64;
            mmio.write32(REG_TSAD0 + (i * 4), slot_pa as u32);
        }

        // Step 6: Configure CAPR read pointer (0 - 0x10 = 0xFFF0 per specification).
        mmio.write16(REG_CAPR, 0xFFF0);

        // Step 7: Configure Receive Configuration (Accept Broadcast, Multicast, Physical Match, Wrap).
        let rcr_val = RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_WRAP;
        mmio.write32(REG_RCR, rcr_val);

        // Step 8: Configure Transmit Configuration (Max DMA burst: 0x03000000).
        mmio.write32(REG_TCR, 0x03000000);

        // Step 9: Subscribe to hardware IRQ if valid.
        if irq != 0 && irq != 0xFF {
            if let Ok(()) = irq::subscribe(irq) {
                // Enable RX and TX interrupts
                mmio.write16(REG_IMR, INT_RX_OK | INT_RX_ERR | INT_TX_OK | INT_TX_ERR);
            }
        }

        // Step 10: Enable Transmitter and Receiver.
        mmio.write8(REG_CHIPCMD, CMD_RX_ENABLE | CMD_TX_ENABLE);

        Ok(Self {
            mmio,
            irq,
            mac,
            _rx_buffer: rx_buffer,
            rx_offset: 0,
            _tx_buffers: tx_buffers,
            tx_cur: 0,
        })
    }

    /// Returns the hardware MAC address.
    pub fn mac(&self) -> MacAddress {
        self.mac
    }

    /// Transmits a network packet via the current TX descriptor.
    pub fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError> {
        if packet.is_empty() || packet.len() > 1792 {
            return Err(SysError::InvalidArgument);
        }

        let slot = self.tx_cur;
        let tsd_reg = REG_TSD0 + (slot * 4);

        // Step 1: Wait for descriptor to become available (OWN bit 13 == 1).
        let mut timeout = 10000;
        while (self.mmio.read32(tsd_reg) & (1 << 13)) == 0 && timeout > 0 {
            timeout -= 1;
        }
        if timeout == 0 {
            return Err(SysError::IoError);
        }

        // Step 2: Copy packet into TX DMA buffer slot.
        let slot_offset = slot * TX_BUFFER_SLOT_SIZE;
        let tx_slice = self._tx_buffers.as_mut_slice();
        tx_slice[slot_offset..slot_offset + packet.len()].copy_from_slice(packet);

        // The packet contents must become globally visible before the TSD
        // doorbell write below can trigger the NIC's DMA engine to read them.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Step 3: Write packet length to TSD (clears OWN bit and triggers transmit).
        // Minimum packet length is 60 bytes.
        let tx_len = packet.len().max(60);
        self.mmio.write32(tsd_reg, tx_len as u32);

        // Step 4: Advance descriptor index.
        self.tx_cur = (self.tx_cur + 1) % 4;

        Ok(())
    }

    /// Polls and copies the next pending packet from the RX ring buffer into `out_buf`.
    ///
    /// Returns `Some(packet_len)` if a packet was received, or `None` if the RX buffer is empty.
    pub fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize> {
        if (self.mmio.read8(REG_CHIPCMD) & CMD_BUF_EMPTY) != 0 {
            return None;
        }

        let rx_slice = self._rx_buffer.as_slice();

        // Step 1: Extract 4-byte RTL8139 packet header.
        let off = self.rx_offset;
        let status = u16::from_le_bytes([rx_slice[off], rx_slice[off + 1]]);
        let length = u16::from_le_bytes([rx_slice[off + 2], rx_slice[off + 3]]) as usize;

        // Step 2: Check packet validity (Receive OK flag).
        if (status & 0x0001) == 0 || !(4..=1792).contains(&length) {
            // Bad packet or descriptor out-of-sync; reset read pointer.
            self.rx_offset = 0;
            self.mmio.write16(REG_CAPR, 0xFFF0);
            return None;
        }

        // Step 3: Extract packet payload (length includes 4-byte CRC).
        let packet_len = length - 4;
        let packet_start = off + 4;
        let packet_end = packet_start + packet_len;

        let copied_len = if packet_end <= rx_slice.len() {
            let to_copy = packet_len.min(out_buf.len());
            out_buf[..to_copy].copy_from_slice(&rx_slice[packet_start..packet_start + to_copy]);
            to_copy
        } else {
            0
        };

        // Step 4: Advance RX ring offset (4-byte aligned).
        self.rx_offset = (off + length + 4 + 3) & !3;
        if self.rx_offset >= RX_RING_SIZE {
            self.rx_offset %= RX_RING_SIZE;
        }

        // Step 5: Update CAPR register.
        let capr_val = (self.rx_offset as u16).wrapping_sub(0x10);
        self.mmio.write16(REG_CAPR, capr_val);

        if copied_len > 0 {
            Some(copied_len)
        } else {
            None
        }
    }

    /// Waits for an IRQ event and acknowledges the interrupt.
    pub fn wait_irq(&mut self, timeout_ms: u32) -> Result<u16, SysError> {
        if self.irq == 0 || self.irq == 0xFF {
            return Ok(0);
        }

        irq::wait(self.irq, timeout_ms)?;
        let status = self.mmio.read16(REG_ISR);
        // Clear ISR bits by writing 1s
        self.mmio.write16(REG_ISR, status);
        let _ = irq::ack(self.irq);

        Ok(status)
    }

    /// Shuts down the receiver and transmitter.
    pub fn shutdown(&mut self) {
        self.mmio.write8(REG_CHIPCMD, 0x00);
        self.mmio.write16(REG_IMR, 0x0000);
    }
}

impl NicDevice for Rtl8139Device {
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
