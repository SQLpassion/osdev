//! Block-device abstraction: a single 512-byte-sector device selected at boot.
//!
//! Filesystems call this facade instead of a concrete driver, so the same FS
//! code runs over ATA PIO (legacy BIOS) or AHCI (UEFI).

use crate::drivers::{ahci, ata};
use crate::sync::spinlock::SpinLock;

/// Fixed sector size for every supported device (matches ATA + AHCI).
pub const SECTOR_SIZE: usize = 512;

/// Maximum sectors transferable in one hardware command (ATA/AHCI count is u8).
const MAX_SECTORS_PER_CMD: u32 = 255;

/// Highest LBA addressable by 28-bit ATA PIO.
const ATA_MAX_LBA: u64 = 0x0FFF_FFFF;

/// Error variants for block device operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// No device selected (init_* never called) or device not ready.
    NotReady,
    /// Caller buffer smaller than count * SECTOR_SIZE.
    BadBuffer,
    /// LBA exceeds what the active device can address.
    OutOfRange,
    /// Underlying driver failed (carries which backend for diagnostics).
    Device,
    /// Operation not supported by the active device (e.g. AHCI writes).
    Unsupported,
}

/// One 512-byte-sector block device.
///
/// lba/count are u64/u32 so the trait outlives 28-bit ATA; adapters clamp/chunk
/// to their hardware limits.
pub trait BlockDevice: Send + Sync {
    /// Read `count` sectors starting at `lba` into `buf`.
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Write `count` sectors starting at `lba` from `buf`.
    fn write_sectors(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError>;

    /// Return the device-specific sector size (defaults to SECTOR_SIZE).
    fn sector_size(&self) -> usize {
        SECTOR_SIZE
    }
}

// ---- ATA adapter (read + write) ----------------------------------------------

/// Adapter for the legacy ATA PIO driver.
pub struct AtaBlockDevice;

impl BlockDevice for AtaBlockDevice {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        // Step 1: Validate buffer size before requesting I/O.
        check_buf(buf.len(), count)?;

        // Step 2: Chunk the request and forward to the ATA driver.
        chunked(lba, count, ATA_MAX_LBA, |chunk_lba, chunk_cnt, off| {
            let bytes = chunk_cnt as usize * SECTOR_SIZE;
            ata::read_sectors(
                &mut buf[off..off + bytes],
                chunk_lba as u32,
                chunk_cnt as u8,
            )
            .map_err(|_| BlockError::Device)
        })
    }

    fn write_sectors(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError> {
        // Step 1: Validate buffer size before requesting I/O.
        check_buf(buf.len(), count)?;

        // Step 2: Chunk the request and forward to the ATA driver.
        chunked(lba, count, ATA_MAX_LBA, |chunk_lba, chunk_cnt, off| {
            let bytes = chunk_cnt as usize * SECTOR_SIZE;
            ata::write_sectors(&buf[off..off + bytes], chunk_lba as u32, chunk_cnt as u8)
                .map_err(|_| BlockError::Device)
        })
    }
}

// ---- AHCI adapter (read-only for now) ----------------------------------------

/// Adapter for the AHCI driver.
pub struct AhciBlockDevice;

impl BlockDevice for AhciBlockDevice {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        // Step 1: Validate buffer size before requesting I/O.
        check_buf(buf.len(), count)?;

        // Step 2: Chunk the request and forward to the AHCI driver.
        // AHCI read_sectors currently takes lba: u32. Keep the u32 ceiling until
        // 48-bit LBA is wired in the driver.
        chunked(lba, count, u32::MAX as u64, |chunk_lba, chunk_cnt, off| {
            let bytes = chunk_cnt as usize * SECTOR_SIZE;
            ahci::read_sectors(
                &mut buf[off..off + bytes],
                chunk_lba as u32,
                chunk_cnt as u8,
            )
            .map_err(|_| BlockError::Device)
        })
    }

    fn write_sectors(&self, _lba: u64, _count: u32, _buf: &[u8]) -> Result<(), BlockError> {
        // Out of scope: AHCI is read-only in this iteration.
        Err(BlockError::Unsupported)
    }
}

/// Helper function to perform bounds checks on the caller's buffer.
fn check_buf(buf_len: usize, count: u32) -> Result<(), BlockError> {
    if buf_len < count as usize * SECTOR_SIZE {
        Err(BlockError::BadBuffer)
    } else {
        Ok(())
    }
}

/// Split a multi-sector request into <=255-sector hardware commands, enforcing
/// the device's LBA ceiling. `op(lba, count, byte_offset)` does one command.
fn chunked(
    lba: u64,
    count: u32,
    max_lba: u64,
    mut op: impl FnMut(u64, u32, usize) -> Result<(), BlockError>,
) -> Result<(), BlockError> {
    // Step 1: If requested count is 0, return early with success.
    if count == 0 {
        return Ok(());
    }

    // Step 2: Verify that the starting LBA and count do not overflow or exceed the max addressable LBA.
    if lba
        .checked_add(count as u64 - 1)
        .is_none_or(|last| last > max_lba)
    {
        return Err(BlockError::OutOfRange);
    }

    // Step 3: Loop through the request, splitting it into chunks up to MAX_SECTORS_PER_CMD.
    let mut remaining = count;
    let mut cur = lba;
    let mut off = 0usize;

    while remaining > 0 {
        let n = remaining.min(MAX_SECTORS_PER_CMD);
        op(cur, n, off)?;
        cur += n as u64;
        off += n as usize * SECTOR_SIZE;
        remaining -= n;
    }

    Ok(())
}

// ---- Global selected device --------------------------------------------------

static ATA_DEVICE: AtaBlockDevice = AtaBlockDevice;
static AHCI_DEVICE: AhciBlockDevice = AhciBlockDevice;

/// The active block device container. Protected by a SpinLock to allow synchronized selection at boot.
static ACTIVE_DEVICE: SpinLock<Option<&'static dyn BlockDevice>> = SpinLock::new(None);

/// Select ATA PIO as the active block device. Call after `ata::init()`.
pub fn init_ata() {
    *ACTIVE_DEVICE.lock() = Some(&ATA_DEVICE);
}

/// Select AHCI as the active block device. Call after `ahci::init()`.
pub fn init_ahci() {
    *ACTIVE_DEVICE.lock() = Some(&AHCI_DEVICE);
}

/// Read `count` sectors at `lba` into `buf` from the active device.
///
/// The active-device container lock is not held across the
/// driver call. The device reference is copied out under the lock and the lock
/// is dropped before invoking `read_sectors`, because ATA PIO waits may yield
/// the current task and a spinlock must never be held across such a yield.
pub fn read_sectors(lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
    // Step 1: Lock the global active device container to copy the static reference.
    // The spinlock is released immediately afterwards to prevent holding it across
    // the driver call, which may yield (like ATA PIO waiting on IRQ14).
    let dev = {
        let guard = ACTIVE_DEVICE.lock();
        (*guard).ok_or(BlockError::NotReady)?
    };

    // Step 2: Invoke the read method on the active block device.
    dev.read_sectors(lba, count, buf)
}

/// Write `count` sectors at `lba`. Errors with `Unsupported` on read-only devices.
///
/// Same as `read_sectors` — the active-device lock is dropped
/// before the backend write call so that yielding device drivers cannot deadlock
/// with other tasks waiting on this lock.
pub fn write_sectors(lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError> {
    // Step 1: Lock the global active device container to copy the static reference.
    // The spinlock is released immediately afterwards to prevent holding it across
    // the driver call, which may yield.
    let dev = {
        let guard = ACTIVE_DEVICE.lock();
        (*guard).ok_or(BlockError::NotReady)?
    };

    // Step 2: Invoke the write method on the active block device.
    dev.write_sectors(lba, count, buf)
}

/// Reset the active block device to None (used for testing state isolation).
pub fn reset_active_device() {
    *ACTIVE_DEVICE.lock() = None;
}
