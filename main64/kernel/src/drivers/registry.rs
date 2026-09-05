//! Kernel-side registry mapping well-known driver names to task ids.
//!
//! Design summary:
//! - Phase 2 of the NIC driver design (`docs/nic_driver_design.md` §4) runs device
//!   drivers as permanent background tasks that Ring-3 applications resolve by a
//!   self-chosen name (e.g. `"nic:rtl8139"`) rather than a task id, since a caller
//!   has no other way to learn which task id a `load <name>.drv` shell command
//!   produced.
//! - This module is the naming layer only: it maps `name -> tid`. It carries no
//!   packet or status data — that is layered on top by later syscalls (`NetSend`/
//!   `NetRecv`, `DrvPublishStatus`/`DrvQuery`).
//! - Follows the same `Vec` behind a `SpinLock` pattern as `driver_db.rs`'s
//!   `DEVICE_BINDINGS`, rather than a fixed array of `Option<T>`.

use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;
use crate::syscall::SyscallError;

/// Maximum number of drivers that may be registered at once.
pub const MAX_DRIVERS: usize = 16;

/// Maximum length, in bytes, of a driver name.
pub const DRIVER_NAME_LEN: usize = 32;

/// One registered driver: its name and the packed task id serving it.
pub struct DriverEntry {
    /// Driver name bytes, left-aligned; only `name[..name_len]` is meaningful.
    pub name: [u8; DRIVER_NAME_LEN],

    /// Number of valid bytes in `name`.
    pub name_len: usize,

    /// Packed (slot, generation) task id — see `pack_task_id` in
    /// `kernel/src/scheduler/roundrobin/manager.rs`. This is the same stable
    /// identity `irq_bridge`/`driver_db` use, so a slot reused by a later,
    /// unrelated task can never be mistaken for this driver.
    pub tid: usize,
}

/// Global driver name registry.
static DRIVER_REGISTRY: SpinLock<Vec<DriverEntry>> = SpinLock::new(Vec::new());

/// Registers `tid` under `name`.
///
/// Fails with `InvalidArg` if `name` is longer than `DRIVER_NAME_LEN`, if the
/// registry already holds `MAX_DRIVERS` entries, or if `name` is already
/// registered (by any tid, including this one). There is no dedicated
/// "AlreadyExists"/"Full" `SyscallError` variant, so all three conditions
/// share `InvalidArg`.
pub fn register(name: &[u8], tid: usize) -> Result<(), SyscallError> {
    // Step 1: reject a name that could never fit an entry's fixed buffer.
    if name.len() > DRIVER_NAME_LEN {
        return Err(SyscallError::InvalidArg);
    }

    let mut registry = DRIVER_REGISTRY.lock();

    // Step 2: enforce the registry capacity before scanning for duplicates,
    // so a full registry is rejected the same way regardless of the name.
    if registry.len() >= MAX_DRIVERS {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: reject a duplicate name. This also rejects a task re-registering
    // its own already-registered name, which is deliberate: DrvRegister is a
    // one-shot call in the driver's startup path, not an idempotent update.
    if registry
        .iter()
        .any(|entry| entry.name[..entry.name_len] == *name)
    {
        return Err(SyscallError::InvalidArg);
    }

    // Step 4: copy the name into a fixed-size buffer and append the entry.
    let mut name_buf = [0u8; DRIVER_NAME_LEN];
    name_buf[..name.len()].copy_from_slice(name);
    registry.push(DriverEntry {
        name: name_buf,
        name_len: name.len(),
        tid,
    });

    Ok(())
}

/// Resolves the packed task id registered under `name`, if any.
pub fn lookup(name: &[u8]) -> Option<usize> {
    DRIVER_REGISTRY
        .lock()
        .iter()
        .find(|entry| entry.name[..entry.name_len] == *name)
        .map(|entry| entry.tid)
}

/// Runs `f` against the registered entry for `tid`, if one exists.
///
/// Closure-based so later steps (packet rings, status snapshots) can attach
/// data to an entry under the same lock acquisition, without this module
/// exposing a raw guard type that would let a caller hold the lock across
/// unrelated work.
#[allow(dead_code)] // Consumed by later Phase-2 steps (NetSend/NetRecv, DrvQuery).
pub fn with_entry_mut<R>(tid: usize, f: impl FnOnce(&mut DriverEntry) -> R) -> Option<R> {
    DRIVER_REGISTRY
        .lock()
        .iter_mut()
        .find(|entry| entry.tid == tid)
        .map(f)
}

/// Removes any entry registered under `tid`.
///
/// Called from the scheduler's `remove_task` — the single choke point reached
/// by both explicit termination and zombie-reaping after a crash — mirroring
/// `driver_db::release_task`/`irq_bridge::release_task`. Without this, a
/// crashed or exited driver would permanently squat its name and `DrvLookup`
/// would keep resolving it to a dead task id.
pub fn release_task(tid: usize) {
    if tid == 0 {
        return;
    }
    DRIVER_REGISTRY.lock().retain(|entry| entry.tid != tid);
}

/// Clears the registry (for unit tests / teardown).
pub fn reset_for_test() {
    DRIVER_REGISTRY.lock().clear();
}
