//! ATA PIO Mode Driver for the Primary ATA Controller
//!
//! Implements 28-bit LBA sector read/write using PIO (Programmed I/O) mode
//! on the primary ATA bus (ports 0x1F0-0x1F7).

use crate::arch::interrupts::{self, SavedRegisters};
use crate::arch::port::{PortByte, PortWord};
use crate::scheduler;
use crate::sync::singlewaitqueue::SingleWaitQueue;
use crate::sync::spinlock::SpinLock;
use crate::sync::waitqueue::WaitQueue;
use crate::sync::waitqueue_adapter;
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
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
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

    /// Set up the command registers for a 28-bit LBA transfer.
    fn setup_command(&self, lba: u32, sector_count: u8, command: u8) {
        // Ensure the device is not busy before programming command registers.
        self.wait_bsy_clear();

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

/// True while one task owns the primary ATA request slot.
static REQUEST_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Waiters blocked while another task owns the ATA request slot.
static REQUEST_WAITQUEUE: WaitQueue = WaitQueue::new();

/// Set by IRQ14 to signal ATA state progress/data readiness.
static IRQ_EVENT_PENDING: AtomicBool = AtomicBool::new(false);

/// Wait queue used by the active ATA request owner while waiting for IRQ14.
static IRQ_WAITQUEUE: SingleWaitQueue = SingleWaitQueue::new();

// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - `controller` serializes all mutable ATA access via `SpinLock`.
// - `initialized` is an atomic flag and does not require external synchronization.
unsafe impl Sync for AtaGlobal {}

/// RAII token for exclusive ATA request ownership.
struct RequestSlotGuard;

impl Drop for RequestSlotGuard {
    fn drop(&mut self) {
        // Step 1: release the global in-flight marker so one waiting request
        // can acquire the controller path.
        REQUEST_IN_FLIGHT.store(false, Ordering::Release);

        // Step 2: wake request waiters outside of any controller lock so
        // blocked tasks can re-contend for the request slot.
        waitqueue_adapter::wake_all_multi(&REQUEST_WAITQUEUE);
    }
}

/// Acquire exclusive ownership of the ATA request path.
///
/// In scheduler context this blocks cooperatively on a wait queue so other
/// tasks can continue to run. In early-boot/test contexts (without scheduler)
/// it falls back to brief spin-waiting.
fn acquire_request_slot() -> RequestSlotGuard {
    loop {
        // Step 1: fast path — try to claim exclusive request ownership.
        if REQUEST_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return RequestSlotGuard;
        }

        // Step 2: decide whether cooperative sleeping is possible in this
        // execution context (scheduler running + current task available).
        let maybe_task_id = if scheduler::is_running() {
            scheduler::current_task_id()
        } else {
            None
        };

        if let Some(task_id) = maybe_task_id {
            // Step 3a: scheduler context — sleep while request slot stays busy.
            // Predicate is rechecked with interrupts disabled by waitqueue adapter.
            if waitqueue_adapter::sleep_if_multi(&REQUEST_WAITQUEUE, task_id, || {
                REQUEST_IN_FLIGHT.load(Ordering::Acquire)
            })
            .should_yield()
            {
                // Hand CPU to current owner or another runnable task.
                scheduler::yield_now();
            }
        } else {
            // Step 3b: early boot/test context — no scheduler sleep available.
            core::hint::spin_loop();
        }
    }
}

#[inline]
fn with_controller<R>(f: impl FnOnce(&AtaPio) -> R) -> R {
    // Serialize direct task-file/data-port access to one caller at a time.
    let ata = PRIMARY_ATA.controller.lock();
    f(&ata)
}

#[inline]
fn status_is_ready(status: StatusRegister) -> Result<bool, AtaError> {
    // Error bits have priority over readiness; callers must fail fast.
    if status.has_error() {
        return Err(AtaError::DeviceError);
    }
    if status.has_fault() {
        return Err(AtaError::DeviceFault);
    }

    Ok(!status.is_busy() && status.is_drq())
}

#[inline]
fn can_sleep_on_irq() -> Option<usize> {
    // Sleeping on IRQ wait queues requires a live scheduler and enabled IRQs.
    if !scheduler::is_running() || !interrupts::are_enabled() {
        return None;
    }

    // Current task ID is required for waitqueue registration.
    scheduler::current_task_id()
}

/// Wait until controller reports `!BSY && DRQ`.
///
/// Primary mode is IRQ-assisted sleeping: the active task sleeps on IRQ14
/// events and re-checks status after every wakeup. In contexts where sleeping
/// is unavailable (tests/early boot), this falls back to short spin polling.
fn wait_ready_or_error() -> Result<(), AtaError> {
    loop {
        // Step 1: sample controller status under controller lock.
        let status = with_controller(AtaPio::read_status);

        // Step 2: terminate on ERR/DF or succeed on !BSY && DRQ.
        if status_is_ready(status)? {
            return Ok(());
        }

        if let Some(task_id) = can_sleep_on_irq() {
            // Step 3a: consume a pending IRQ edge before sleeping.
            // If one is already queued, re-check status immediately.
            if IRQ_EVENT_PENDING.swap(false, Ordering::AcqRel) {
                continue;
            }

            // Step 3b: sleep until IRQ14 marks a new ATA event.
            if waitqueue_adapter::sleep_if_single(&IRQ_WAITQUEUE, task_id, || {
                !IRQ_EVENT_PENDING.load(Ordering::Acquire)
            })
            .should_yield()
            {
                // Yield cooperatively so IRQ worker/scheduler can progress.
                scheduler::yield_now();
            }
        } else {
            // Step 3c: fallback for contexts without scheduler/IRQs.
            core::hint::spin_loop();
        }
    }
}

/// IRQ14 top-half handler for the primary ATA controller.
///
/// Responsibilities of this handler are intentionally minimal and bounded:
/// - acknowledge ATA progress to the waiting request path via
///   `IRQ_EVENT_PENDING`,
/// - wake the single requester sleeping on `IRQ_WAITQUEUE`,
/// - return the unchanged trap frame pointer to the IRQ dispatcher.
///
/// Design constraints:
/// - No data-port PIO transfer is performed here.
///   All 16-bit sector reads/writes remain in `read_sectors`/`write_sectors`
///   after the task wakes and re-checks controller state.
/// - Exactly one ATA request may be active (`REQUEST_IN_FLIGHT`), therefore
///   single-waiter wakeup is sufficient and avoids wakeup storms.
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

    // Step 2: wake exactly one ATA waiter (single active request owner).
    waitqueue_adapter::wake_all_single(&IRQ_WAITQUEUE);

    // Step 3: continue with current trap frame; scheduler may switch later.
    frame as *mut SavedRegisters
}

/// Initialize the primary ATA controller.
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

    let total_bytes = sector_count as usize * SECTOR_SIZE;
    assert!(
        buffer.len() >= total_bytes,
        "ATA read buffer too small: need {} bytes, got {}",
        total_bytes,
        buffer.len()
    );

    // Step 1: serialize full request lifetime without holding a spinlock.
    // The slot can be held across scheduler sleeps, unlike SpinLock guards.
    let _request = acquire_request_slot();

    // Clear stale IRQ edge from any prior request before issuing a command.
    IRQ_EVENT_PENDING.store(false, Ordering::Release);

    // Step 2: program task-file registers.
    with_controller(|ata| ata.setup_command(lba, sector_count, ATA_CMD_READ_SECTORS));

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

    let total_bytes = sector_count as usize * SECTOR_SIZE;
    assert!(
        buffer.len() >= total_bytes,
        "ATA write buffer too small: need {} bytes, got {}",
        total_bytes,
        buffer.len()
    );

    // Step 1: serialize full request lifetime without holding a spinlock.
    let _request = acquire_request_slot();

    // Clear stale IRQ edge from any prior request before issuing a command.
    IRQ_EVENT_PENDING.store(false, Ordering::Release);

    // Step 2: program task-file registers.
    with_controller(|ata| ata.setup_command(lba, sector_count, ATA_CMD_WRITE_SECTORS));

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

    Ok(())
}
