//! ATA PIO Mode Driver for the Primary ATA Controller
//!
//! Implements 28-bit LBA sector read/write using PIO (Programmed I/O) mode
//! on the primary ATA bus (ports 0x1F0-0x1F7).

use crate::arch::interrupts::{self, SavedRegisters};
use crate::arch::port::{PortByte, PortWord};
use crate::scheduler;
use crate::sync::request_slot::InFlightSlot;
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

/// Primary ATA controller *control block* base I/O port.
///
/// This is a genuinely separate I/O port range from the command block
/// (`PRIMARY_BASE`, ports 0x1F0-0x1F7) per the ATA/ATAPI specification -
/// it is not reachable as an offset from `PRIMARY_BASE`. The "alternate
/// status" register lives here; unlike the regular status register at
/// `PRIMARY_BASE + STATUS_COMMAND_OFFSET`, reading it has no side effect
/// of acknowledging/clearing a pending IRQ, which makes it safe to poll
/// purely for timing purposes.
const PRIMARY_CONTROL_BASE: u16 = 0x3F6;

/// ATA PIO commands.
const ATA_CMD_READ_SECTORS: u8 = 0x20;
const ATA_CMD_WRITE_SECTORS: u8 = 0x30;

/// Flush the drive's write cache to stable media (28-bit command set).
const ATA_CMD_CACHE_FLUSH: u8 = 0xE7;

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

/// Maximum number of polling iterations before ATA waits time out.
const ATA_POLL_TIMEOUT_ITERATIONS: u32 = 10_000;

/// Maximum number of polling iterations before a CACHE FLUSH completion wait
/// times out.
///
/// A drive can legitimately take much longer to persist a full write cache
/// to stable media than to complete a single sector's DRQ handshake, so
/// FLUSH CACHE gets a more generous budget than [`ATA_POLL_TIMEOUT_ITERATIONS`]
/// instead of sharing the tight per-sector one, which would risk a spurious
/// `AtaError::Timeout` on real hardware with a large dirty write cache.
const ATA_CACHE_FLUSH_TIMEOUT_ITERATIONS: u32 = 100_000;

/// Errors that can occur during ATA operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    /// The drive reported an error (ERR bit set in status).
    DeviceError,

    /// The drive reported a fault (DF bit set in status).
    DeviceFault,

    /// The LBA exceeds the 28-bit limit (0x0FFFFFFF).
    LbaOutOfRange,

    /// The controller did not leave the expected state before timeout.
    Timeout,
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
    alt_status: PortByte,
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
            // The control block lives at a fixed, separate base address
            // per the ATA spec, not at an offset within the command block.
            alt_status: PortByte::new(PRIMARY_CONTROL_BASE),
        }
    }

    /// Read the status register.
    fn read_status(&self) -> StatusRegister {
        // SAFETY:
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        // - Reading ATA status uses the controller I/O port for this device.
        // - `self.status_cmd` was constructed from the ATA base port.
        unsafe { StatusRegister(self.status_cmd.read()) }
    }

    /// Burn ~400 ns after issuing a command by reading the alternate status
    /// register four times, discarding the value.
    ///
    /// This is the well-known, industry-wide "400 ns settle" convention:
    /// the drive is allowed to take a few hundred nanoseconds after a
    /// command-register write before it asserts BSY, so the very first
    /// status sample taken right after the write can observe stale
    /// `!BSY && DRQ` from the *previous* command. Reading the alternate
    /// status register (rather than the regular status register) avoids
    /// accidentally acknowledging/clearing a pending IRQ while doing so.
    fn settle_after_command(&self) {
        // SAFETY:
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        // - Reading the alternate status port has no side effect (no IRQ ack) and is used here purely to burn time.
        // - `self.alt_status` was constructed from the documented primary control-block base port.
        for _ in 0..4 {
            unsafe {
                let _ = self.alt_status.read();
            }
        }
    }

    /// Busy-wait until the BSY flag is cleared.
    fn wait_bsy_clear(&self) -> Result<(), AtaError> {
        let mut timeout = ATA_POLL_TIMEOUT_ITERATIONS;

        while self.read_status().is_busy() {
            // Abort after bounded retries to avoid infinite hangs on faulty hardware.
            if timeout == 0 {
                return Err(AtaError::Timeout);
            }
            timeout -= 1;
            core::hint::spin_loop();
        }

        Ok(())
    }

    /// Set up the command registers for a 28-bit LBA transfer.
    fn setup_command(&self, lba: u32, sector_count: u8, command: u8) -> Result<(), AtaError> {
        // Ensure the device is not busy before programming command registers.
        self.wait_bsy_clear()?;

        // Program transfer count and 28-bit LBA address, then issue command.
        // SAFETY:
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        // - Writes target ATA task-file registers on the configured bus.
        // - Caller guarantees `lba` is 28-bit and command byte is valid.
        unsafe {
            self.sector_count.write(sector_count);
            self.lba_low.write(lba as u8);
            self.lba_mid.write((lba >> 8) as u8);
            self.lba_high.write((lba >> 16) as u8);
            self.drive_head
                .write(DRIVE_SELECT_MASTER_LBA | ((lba >> 24) as u8 & 0x0F));
            self.status_cmd.write(command);
        }

        // Give the drive time to assert BSY before the caller trusts the
        // first real-status sample. Without this, a fast real CPU can read
        // stale `!BSY && DRQ` left over from the previous command.
        self.settle_after_command();

        Ok(())
    }

    /// Issue CACHE FLUSH (0xE7) to persist the drive's write cache to
    /// stable media.
    ///
    /// Per the ATA spec, re-programming the LBA/sector-count/drive-head
    /// registers is not required for FLUSH CACHE - the preceding
    /// `setup_command` call already selected the target drive, and only the
    /// command byte needs to be written here.
    ///
    /// Waits for BSY to clear before writing the command byte, mirroring
    /// `setup_command`'s own precondition check, rather than relying on the
    /// caller to have already ensured the device is idle. This keeps the
    /// method self-contained and safe to call correctly on its own, even
    /// though today's only call site already waits beforehand.
    fn issue_cache_flush(&self) -> Result<(), AtaError> {
        self.wait_bsy_clear()?;

        // SAFETY:
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        // - Writes the ATA command register on the configured bus.
        // - The drive/head register still selects the correct target drive from the preceding `setup_command` call.
        unsafe {
            self.status_cmd.write(ATA_CMD_CACHE_FLUSH);
        }

        // Same settle rationale as after any other command-register write.
        self.settle_after_command();

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

/// Primitive for exclusive ATA request ownership.
static REQUEST_SLOT: InFlightSlot = InFlightSlot::new();

/// Set by IRQ14 to signal ATA state progress/data readiness.
static IRQ_EVENT_PENDING: AtomicBool = AtomicBool::new(false);

// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - `controller` serializes all mutable ATA access via `SpinLock`.
// - `initialized` is an atomic flag and does not require external synchronization.
unsafe impl Sync for AtaGlobal {}

fn with_controller<R>(f: impl FnOnce(&AtaPio) -> R) -> R {
    // Serialize direct task-file/data-port access to one caller at a time.
    let ata = PRIMARY_ATA.controller.lock();
    f(&ata)
}

fn can_sleep_on_irq() -> Option<usize> {
    // Sleeping on IRQ wait queues requires a live scheduler and enabled IRQs.
    if !scheduler::is_running() || !interrupts::are_enabled() {
        return None;
    }

    // Current task ID is required for waitqueue registration.
    scheduler::current_task_id()
}

/// Shared polling core for every "wait for a controller status condition"
/// loop in this driver.
///
/// Samples the status register under the controller lock, fails fast on
/// ERR/DF (checked before `condition`, since error bits take priority over
/// whatever positive condition the caller is waiting for), and otherwise
/// calls `condition` to decide whether the awaited state has been reached.
/// Bounded by `timeout_iterations` polling rounds; between rounds, uses
/// cooperative IRQ-hinted polling:
/// - IRQ14 sets `IRQ_EVENT_PENDING` as a wake hint.
/// - The requester re-checks status in a loop and yields between checks.
///
/// This avoids deadlocks on hardware/controllers that do not reliably deliver
/// ATA IRQ edges for every intermediate device state transition.
fn poll_status_until(
    timeout_iterations: u32,
    condition: impl Fn(StatusRegister) -> bool,
) -> Result<(), AtaError> {
    let mut timeout = timeout_iterations;

    loop {
        // Step 1: sample controller status under controller lock.
        let status = with_controller(AtaPio::read_status);

        // Step 2: error bits have priority over the awaited condition;
        // callers must fail fast.
        if status.has_error() {
            return Err(AtaError::DeviceError);
        }
        if status.has_fault() {
            return Err(AtaError::DeviceFault);
        }
        if condition(status) {
            return Ok(());
        }

        // Step 3: abort after bounded retries to prevent unbounded kernel hangs.
        if timeout == 0 {
            return Err(AtaError::Timeout);
        }

        timeout -= 1;

        if can_sleep_on_irq().is_some() {
            // Step 4a: consume a pending IRQ edge before yielding.
            // If one is already queued, re-check status immediately.
            if IRQ_EVENT_PENDING.swap(false, Ordering::AcqRel) {
                continue;
            }

            // Step 4b: no pending IRQ hint; yield once and poll again.
            // This keeps the system responsive even if an IRQ edge is missed.
            scheduler::yield_now();
        } else {
            // Step 4c: fallback for contexts without scheduler/IRQs.
            core::hint::spin_loop();
        }
    }
}

/// Wait until controller reports `!BSY && DRQ`, i.e. the per-sector
/// read/write data-request handshake.
fn wait_ready_or_error() -> Result<(), AtaError> {
    poll_status_until(ATA_POLL_TIMEOUT_ITERATIONS, |status| {
        !status.is_busy() && status.is_drq()
    })
}

/// Wait for command completion (BSY clear) without requiring DRQ.
///
/// Unlike [`wait_ready_or_error`], which is used for the read/write
/// per-sector data-request handshake, this is used after the *last* sector
/// of a write and after CACHE FLUSH, where DRQ is not expected to be set
/// again. `timeout_iterations` is caller-supplied so a comparatively slow
/// CACHE FLUSH (real drives can take much longer to flush a write cache
/// than to transfer a single sector) can be given a more generous budget
/// than the tight per-sector wait, instead of risking a spurious timeout.
fn wait_completion_or_error(timeout_iterations: u32) -> Result<(), AtaError> {
    poll_status_until(timeout_iterations, |status| !status.is_busy())
}

/// IRQ14 top-half handler for the primary ATA controller.
///
/// Responsibilities of this handler are intentionally minimal and bounded:
/// - acknowledge ATA progress to the waiting request path via
///   `IRQ_EVENT_PENDING`,
/// - return the unchanged trap frame pointer to the IRQ dispatcher.
///
/// Design constraints:
/// - No data-port PIO transfer is performed here.
///   All 16-bit sector reads/writes remain in `read_sectors`/`write_sectors`
///   after the task wakes and re-checks controller state.
/// - This function must not block or take long-running locks because it runs
///   in interrupt context.
///
/// Ordering contract:
/// - Store to `IRQ_EVENT_PENDING` uses `Release`.
/// - Wait side consumes with `AcqRel`/`Acquire` before sleeping/rechecking.
/// - This guarantees the requester observes the IRQ event and does not miss
///   the wakeup edge.
fn primary_ata_irq_handler(_vector: u8, frame: &mut SavedRegisters) -> *mut SavedRegisters {
    // Step 1: publish one "ATA progressed" event for the active requester.
    IRQ_EVENT_PENDING.store(true, Ordering::Release);

    // Step 2: continue with current trap frame; scheduler may switch later.
    frame as *mut SavedRegisters
}

/// Initialize the primary ATA controller.
/// Returns whether a drive responds on the primary ATA channel.
///
/// Reads the primary status register (`PRIMARY_BASE + STATUS_COMMAND_OFFSET`, i.e. 0x1F7). An
/// empty/disconnected channel floats high and reads back 0xFF; any other value indicates that a
/// drive is present. This is used to distinguish a legacy BIOS boot (which always exposes a
/// legacy IDE disk) from a UEFI boot (no legacy IDE), without a dedicated boot-source flag, and
/// does not require [`init`] to have run (a plain status read has no lasting side effects here).
pub fn primary_present() -> bool {
    // SAFETY:
    // - Hardware port I/O is outside Rust's memory-safety guarantees.
    // - 0x1F7 is the documented primary ATA status register; a read is side-effect-free before init.
    let status = unsafe { PortByte::new(PRIMARY_BASE + STATUS_COMMAND_OFFSET).read() };
    status != 0xFF
}

pub fn init() {
    // Register ATA IRQ before exposing `initialized=true` so new requests
    // cannot miss handler installation.
    interrupts::register_irq_handler(
        interrupts::IRQ14_PRIMARY_ATA_VECTOR,
        primary_ata_irq_handler,
    );

    // Publish readiness for external callers.
    PRIMARY_ATA.initialized.store(true, Ordering::Release);
}

/// Read sectors from the global primary ATA drive instance.
///
/// Lifecycle contract:
/// - [`init`] must be called before any ATA I/O call.
///
/// Delegates to [`AtaPio::read_sectors`] for transfer semantics.
pub fn read_sectors(buffer: &mut [u8], lba: u32, sector_count: u8) -> Result<(), AtaError> {
    // Step 0: lifecycle guard.
    assert!(
        PRIMARY_ATA.initialized.load(Ordering::Acquire),
        "ATA driver not initialized"
    );

    // Step 1: validate user-provided geometry before touching hardware.
    if lba > 0x0FFF_FFFF {
        return Err(AtaError::LbaOutOfRange);
    }

    // ATA interprets sector-count 0 as 256 sectors. Reject the ambiguous
    // value early so the controller is never programmed with it.
    if sector_count == 0 {
        return Ok(());
    }

    let total_bytes = sector_count as usize * SECTOR_SIZE;
    assert!(
        buffer.len() >= total_bytes,
        "ATA read buffer too small: need {} bytes, got {}",
        total_bytes,
        buffer.len()
    );

    // Step 1: serialize full request lifetime without holding a spinlock.
    // The slot can be held across scheduler sleeps, unlike SpinLock guards.
    let _request = REQUEST_SLOT.acquire();

    // Clear stale IRQ edge from any prior request before issuing a command.
    IRQ_EVENT_PENDING.store(false, Ordering::Release);

    // Step 2: program task-file registers.
    with_controller(|ata| ata.setup_command(lba, sector_count, ATA_CMD_READ_SECTORS))?;

    // Step 3: transfer sectors.
    for sector in 0..sector_count as usize {
        // Wait until this sector transfer is accepted by device (or fails).
        wait_ready_or_error()?;

        let sector_offset = sector * SECTOR_SIZE;
        with_controller(|ata| {
            for word_idx in 0..WORDS_PER_SECTOR {
                // SAFETY:
                // - This requires `unsafe` because hardware port I/O is outside Rust safety checks.
                // - Controller state is `!BSY && DRQ` for this sector.
                // - The active request slot guarantees exclusive ATA data-port ownership.
                let word = unsafe { ata.data.read() };

                // Copy one PIO word into destination buffer in little-endian layout.
                let byte_offset = sector_offset + word_idx * 2;
                buffer[byte_offset] = word as u8;
                buffer[byte_offset + 1] = (word >> 8) as u8;
            }
        });
    }

    Ok(())
}

/// Write sectors to the global primary ATA drive instance.
///
/// Lifecycle contract:
/// - [`init`] must be called before any ATA I/O call.
///
/// Delegates to [`AtaPio::write_sectors`] for transfer semantics.
pub fn write_sectors(buffer: &[u8], lba: u32, sector_count: u8) -> Result<(), AtaError> {
    // Step 0: lifecycle guard.
    assert!(
        PRIMARY_ATA.initialized.load(Ordering::Acquire),
        "ATA driver not initialized"
    );

    // Step 1: validate caller-provided addressing and buffer size.
    if lba > 0x0FFF_FFFF {
        return Err(AtaError::LbaOutOfRange);
    }

    // ATA interprets sector-count 0 as 256 sectors. Reject the ambiguous
    // value early so the controller is never programmed with it.
    if sector_count == 0 {
        return Ok(());
    }

    let total_bytes = sector_count as usize * SECTOR_SIZE;
    assert!(
        buffer.len() >= total_bytes,
        "ATA write buffer too small: need {} bytes, got {}",
        total_bytes,
        buffer.len()
    );

    // Step 1: serialize full request lifetime without holding a spinlock.
    let _request = REQUEST_SLOT.acquire();

    // Clear stale IRQ edge from any prior request before issuing a command.
    IRQ_EVENT_PENDING.store(false, Ordering::Release);

    // Step 2: program task-file registers.
    with_controller(|ata| ata.setup_command(lba, sector_count, ATA_CMD_WRITE_SECTORS))?;

    // Step 3: transfer sectors.
    for sector in 0..sector_count as usize {
        // Wait until device requests the next sector payload.
        wait_ready_or_error()?;

        let sector_offset = sector * SECTOR_SIZE;
        with_controller(|ata| {
            for word_idx in 0..WORDS_PER_SECTOR {
                // Pack two bytes into one 16-bit PIO word (little-endian).
                let byte_offset = sector_offset + word_idx * 2;
                let word = (buffer[byte_offset] as u16) | ((buffer[byte_offset + 1] as u16) << 8);

                // SAFETY:
                // - This requires `unsafe` because hardware port I/O is outside Rust safety checks.
                // - Controller state is `!BSY && DRQ` for this sector.
                // - The active request slot guarantees exclusive ATA data-port ownership.
                unsafe {
                    ata.data.write(word);
                }
            }
        });
    }

    // Step 4: wait for the final sector's completion (BSY clear) and bail
    // out on a device-reported error/fault before trusting the write, or
    // before touching the write cache with a FLUSH command.
    wait_completion_or_error(ATA_POLL_TIMEOUT_ITERATIONS)?;

    // Step 5: flush the drive's write cache so the data just written is
    // durable on stable media, then wait for the flush itself to complete.
    // The flush completion wait gets a larger, dedicated timeout budget
    // since persisting a full write cache can legitimately take much
    // longer than an ordinary per-sector handshake.
    with_controller(|ata| ata.issue_cache_flush())?;
    wait_completion_or_error(ATA_CACHE_FLUSH_TIMEOUT_ITERATIONS)?;

    Ok(())
}
