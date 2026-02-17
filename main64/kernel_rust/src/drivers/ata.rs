//! ATA PIO Mode Driver for the Primary ATA Controller
//!
//! Implements 28-bit LBA sector read/write using PIO (Programmed I/O) mode
//! on the primary ATA bus (ports 0x1F0-0x1F7).

use crate::arch::port::{PortByte, PortWord};
use crate::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

/// Bytes per sector on an ATA disk.
const SECTOR_SIZE: usize = 512;

/// Number of 16-bit words per sector (512 / 2 = 256).
const WORDS_PER_SECTOR: usize = SECTOR_SIZE / 2;

/// Primary ATA controller base I/O port.
const PRIMARY_BASE: u16 = 0x1F0;

// Primary ATA controller port offsets from base
const DATA_PORT_OFFSET: u16 = 0;
const SECTOR_COUNT_OFFSET: u16 = 2;
const LBA_LOW_OFFSET: u16 = 3;
const LBA_MID_OFFSET: u16 = 4;
const LBA_HIGH_OFFSET: u16 = 5;
const DRIVE_HEAD_OFFSET: u16 = 6;
const STATUS_COMMAND_OFFSET: u16 = 7;

/// ATA PIO commands.
const ATA_CMD_READ_SECTORS: u8 = 0x20;
const ATA_CMD_WRITE_SECTORS: u8 = 0x30;

/// Drive select byte: master drive, LBA mode.
const DRIVE_SELECT_MASTER_LBA: u8 = 0xE0;

/// ATA status register bits.
#[derive(Clone, Copy)]
struct StatusRegister(u8);

impl StatusRegister {
    const BSY: u8 = 0x80;
    const DRQ: u8 = 0x08;
    const DF: u8 = 0x20;
    const ERR: u8 = 0x01;

    fn is_busy(self) -> bool {
        self.0 & Self::BSY != 0
    }

    fn is_drq(self) -> bool {
        self.0 & Self::DRQ != 0
    }

    fn has_fault(self) -> bool {
        self.0 & Self::DF != 0
    }

    fn has_error(self) -> bool {
        self.0 & Self::ERR != 0
    }
}

/// Errors that can occur during ATA operations.
#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    /// The drive reported an error (ERR bit set in status).
    DeviceError,
    /// The drive reported a fault (DF bit set in status).
    DeviceFault,
    /// The LBA exceeds the 28-bit limit (0x0FFFFFFF).
    LbaOutOfRange,
}

/// ATA PIO driver for one ATA bus.
pub struct AtaPio {
    data: PortWord,
    sector_count: PortByte,
    lba_low: PortByte,
    lba_mid: PortByte,
    lba_high: PortByte,
    drive_head: PortByte,
    status_cmd: PortByte,
}

impl AtaPio {
    /// Create a new ATA PIO driver for the given base port.
    pub const fn new(base: u16) -> Self {
        Self {
            data: PortWord::new(base + DATA_PORT_OFFSET),
            sector_count: PortByte::new(base + SECTOR_COUNT_OFFSET),
            lba_low: PortByte::new(base + LBA_LOW_OFFSET),
            lba_mid: PortByte::new(base + LBA_MID_OFFSET),
            lba_high: PortByte::new(base + LBA_HIGH_OFFSET),
            drive_head: PortByte::new(base + DRIVE_HEAD_OFFSET),
            status_cmd: PortByte::new(base + STATUS_COMMAND_OFFSET),
        }
    }

    /// Read the status register.
    fn read_status(&self) -> StatusRegister {
        // SAFETY:
        // - Reading ATA status uses the controller I/O port for this device.
        // - `self.status_cmd` was constructed from the ATA base port.
        unsafe { StatusRegister(self.status_cmd.read()) }
    }

    /// Busy-wait until the BSY flag is cleared.
    fn wait_bsy_clear(&self) {
        while self.read_status().is_busy() {
            core::hint::spin_loop();
        }
    }

    /// Busy-wait until DRQ is set (data ready to transfer).
    /// Also checks for error/fault conditions.
    fn wait_drq(&self) -> Result<(), AtaError> {
        loop {
            let status = self.read_status();

            if status.has_error() {
                return Err(AtaError::DeviceError);
            }

            if status.has_fault() {
                return Err(AtaError::DeviceFault);
            }

            if !status.is_busy() && status.is_drq() {
                return Ok(());
            }

            core::hint::spin_loop();
        }
    }

    /// Set up the command registers for a 28-bit LBA transfer.
    fn setup_command(&self, lba: u32, sector_count: u8, command: u8) {
        // Ensure the device is not busy before programming command registers.
        self.wait_bsy_clear();

        // Program transfer count and 28-bit LBA address, then issue command.
        unsafe {
            // SAFETY:
            // - Writes target ATA task-file registers on the configured bus.
            // - Caller guarantees `lba` is 28-bit and command byte is valid.
            self.sector_count.write(sector_count);
            self.lba_low.write(lba as u8);
            self.lba_mid.write((lba >> 8) as u8);
            self.lba_high.write((lba >> 16) as u8);
            self.drive_head
                .write(DRIVE_SELECT_MASTER_LBA | ((lba >> 24) as u8 & 0x0F));
            self.status_cmd.write(command);
        }
    }

    /// Read `sector_count` consecutive sectors starting at `lba` into `buffer`.
    ///
    /// Uses ATA PIO `READ SECTORS` (0x20) and transfers one 16-bit word at a
    /// time from the data port into `buffer` (little-endian byte order).
    ///
    /// Contract:
    /// - `lba` must fit in 28-bit addressing (`<= 0x0FFF_FFFF`).
    /// - `buffer.len()` must be at least `sector_count as usize * 512`.
    /// - On success, exactly `sector_count * 512` bytes are written to
    ///   the front of `buffer`.
    ///
    /// Errors:
    /// - [`AtaError::LbaOutOfRange`] if `lba` exceeds 28-bit range.
    /// - [`AtaError::DeviceError`] if the controller reports `ERR`.
    /// - [`AtaError::DeviceFault`] if the controller reports `DF`.
    ///
    /// Panics:
    /// - If `buffer` is too small for the requested transfer.
    ///
    /// Execution model:
    /// - Synchronous and busy-waiting; the call spins until `BSY` clears and
    ///   `DRQ` is asserted for each transferred sector.
    pub fn read_sectors(
        &self,
        buffer: &mut [u8],
        lba: u32,
        sector_count: u8,
    ) -> Result<(), AtaError> {
        if lba > 0x0FFF_FFFF {
            return Err(AtaError::LbaOutOfRange);
        }

        let total_bytes = sector_count as usize * SECTOR_SIZE;
        assert!(
            buffer.len() >= total_bytes,
            "ATA read buffer too small: need {} bytes, got {}",
            total_bytes,
            buffer.len()
        );

        // Program controller registers and issue READ SECTORS command.
        self.setup_command(lba, sector_count, ATA_CMD_READ_SECTORS);

        for sector in 0..sector_count as usize {
            // For every sector wait until controller is ready to transfer.
            self.wait_bsy_clear();
            self.wait_drq()?;

            let sector_offset = sector * SECTOR_SIZE;

            for word_idx in 0..WORDS_PER_SECTOR {
                // Read one 16-bit PIO word and store it little-endian in buffer.
                let word = unsafe {
                    // SAFETY:
                    // - DRQ was observed for this sector before entering loop.
                    // - Reading from ATA data port consumes one data word.
                    self.data.read()
                };

                let byte_offset = sector_offset + word_idx * 2;
                buffer[byte_offset] = word as u8;
                buffer[byte_offset + 1] = (word >> 8) as u8;
            }
        }

        Ok(())
    }

    /// Write `sector_count` consecutive sectors starting at `lba` from `buffer`.
    ///
    /// Uses ATA PIO `WRITE SECTORS` (0x30) and writes one 16-bit word at a
    /// time to the data port, packing pairs of bytes from `buffer` as
    /// little-endian words.
    ///
    /// Contract:
    /// - `lba` must fit in 28-bit addressing (`<= 0x0FFF_FFFF`).
    /// - `buffer.len()` must be at least `sector_count as usize * 512`.
    /// - The first `sector_count * 512` bytes of `buffer` are written.
    ///
    /// Errors:
    /// - [`AtaError::LbaOutOfRange`] if `lba` exceeds 28-bit range.
    /// - [`AtaError::DeviceError`] if the controller reports `ERR`.
    /// - [`AtaError::DeviceFault`] if the controller reports `DF`.
    ///
    /// Panics:
    /// - If `buffer` is too small for the requested transfer.
    ///
    /// Execution model:
    /// - Synchronous and busy-waiting; the call spins until `BSY` clears and
    ///   `DRQ` is asserted for each transferred sector.
    pub fn write_sectors(&self, buffer: &[u8], lba: u32, sector_count: u8) -> Result<(), AtaError> {
        if lba > 0x0FFF_FFFF {
            return Err(AtaError::LbaOutOfRange);
        }

        let total_bytes = sector_count as usize * SECTOR_SIZE;

        assert!(
            buffer.len() >= total_bytes,
            "ATA write buffer too small: need {} bytes, got {}",
            total_bytes,
            buffer.len()
        );

        // Program controller registers and issue WRITE SECTORS command.
        self.setup_command(lba, sector_count, ATA_CMD_WRITE_SECTORS);

        for sector in 0..sector_count as usize {
            // For every sector wait until controller requests write data.
            self.wait_bsy_clear();
            self.wait_drq()?;

            let sector_offset = sector * SECTOR_SIZE;

            for word_idx in 0..WORDS_PER_SECTOR {
                let byte_offset = sector_offset + word_idx * 2;
                let word = (buffer[byte_offset] as u16) | ((buffer[byte_offset + 1] as u16) << 8);

                unsafe {
                    // SAFETY:
                    // - DRQ was observed for this sector before entering loop.
                    // - Writing to ATA data port sends one data word.
                    self.data.write(word);
                }
            }
        }

        Ok(())
    }
}

/// Global primary ATA controller instance.
struct AtaGlobal {
    controller: SpinLock<AtaPio>,
    initialized: AtomicBool,
}

static PRIMARY_ATA: AtaGlobal = AtaGlobal {
    controller: SpinLock::new(AtaPio::new(PRIMARY_BASE)),
    initialized: AtomicBool::new(false),
};

// Safety: AtaGlobal is Sync because SpinLock<AtaPio> is Sync and AtomicBool is Sync.
unsafe impl Sync for AtaGlobal {}

/// Initialize the primary ATA controller.
pub fn init() {
    PRIMARY_ATA.initialized.store(true, Ordering::Release);
}

/// Read sectors from the global primary ATA drive instance.
///
/// Lifecycle contract:
/// - [`init`] must be called before any ATA I/O call.
///
/// Delegates to [`AtaPio::read_sectors`] for transfer semantics.
pub fn read_sectors(buffer: &mut [u8], lba: u32, sector_count: u8) -> Result<(), AtaError> {
    assert!(
        PRIMARY_ATA.initialized.load(Ordering::Acquire),
        "ATA driver not initialized"
    );

    // Serialize access to the single primary ATA controller.
    let ata = PRIMARY_ATA.controller.lock();
    ata.read_sectors(buffer, lba, sector_count)
}

/// Write sectors to the global primary ATA drive instance.
///
/// Lifecycle contract:
/// - [`init`] must be called before any ATA I/O call.
///
/// Delegates to [`AtaPio::write_sectors`] for transfer semantics.
pub fn write_sectors(buffer: &[u8], lba: u32, sector_count: u8) -> Result<(), AtaError> {
    assert!(
        PRIMARY_ATA.initialized.load(Ordering::Acquire),
        "ATA driver not initialized"
    );

    // Serialize access to the single primary ATA controller.
    let ata = PRIMARY_ATA.controller.lock();
    ata.write_sectors(buffer, lba, sector_count)
}
