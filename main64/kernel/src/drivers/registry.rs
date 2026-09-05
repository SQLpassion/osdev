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
//! - Also owns the per-driver TX/RX packet rings (`NetSend`/`NetRecv`,
//!   Phase 2 Step 2): the kernel copies packets in and out via syscalls
//!   rather than exposing shared memory between two Ring-3 address spaces.

use alloc::vec;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;
use crate::syscall::SyscallError;

/// Maximum number of drivers that may be registered at once.
pub const MAX_DRIVERS: usize = 16;

/// Maximum length, in bytes, of a driver name.
pub const DRIVER_NAME_LEN: usize = 32;

/// Number of packets a `PacketRing` can hold before `push` starts rejecting.
pub const RING_CAPACITY: usize = 32;

/// Maximum length, in bytes, of a single queued packet — matches `lib_net`'s
/// Ethernet MTU assumption (1500-byte payload + 14/18-byte header rounded up).
pub const MAX_PACKET_LEN: usize = 1536;

/// One queued packet inside a [`PacketRing`].
#[derive(Clone)]
struct PacketSlot {
    len: u16,
    data: [u8; MAX_PACKET_LEN],
}

/// A bounded FIFO queue of packets, used for one direction (App→Driver or
/// Driver→App) of a single driver's channel.
///
/// Backed by a heap-allocated `Vec`, **not** a fixed `[PacketSlot;
/// RING_CAPACITY]` array: each slot is ~1.5 KB, so `RING_CAPACITY` of them is
/// ~49 KB — large enough that constructing it as a stack temporary (which a
/// fixed-size array field risks, since `DriverEntry { ..., tx_ring: PacketRing
/// { slots: [...] } }` would build the whole struct on the stack before
/// `Vec::push` moves it onto the registry's heap) could overflow a task's
/// 64 KiB stack (`scheduler::roundrobin::TASK_STACK_SIZE`). `vec![x; n]`'s
/// `from_elem` fill path writes directly into the heap allocation instead.
struct PacketRing {
    slots: Vec<PacketSlot>, // always exactly RING_CAPACITY elements
    head: usize,            // next slot index to pop
    tail: usize,            // next slot index to push
    count: usize,           // number of currently queued packets
}

impl PacketRing {
    fn new() -> Self {
        Self {
            slots: vec![
                PacketSlot {
                    len: 0,
                    data: [0u8; MAX_PACKET_LEN],
                };
                RING_CAPACITY
            ],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Pushes `data` into the ring. Never blocks: a full ring is
    /// backpressure to the sender, not something worth blocking a producer
    /// over — packets are fire-and-forget per `docs/nic_driver_design.md`
    /// §4.5. Fails with `InvalidArg` if the ring is full or `data` is larger
    /// than `MAX_PACKET_LEN` (the latter defends this function even though
    /// callers are expected to reject an oversized packet earlier).
    fn push(&mut self, data: &[u8]) -> Result<(), SyscallError> {
        if data.len() > MAX_PACKET_LEN || self.count >= RING_CAPACITY {
            return Err(SyscallError::InvalidArg);
        }
        let slot = &mut self.slots[self.tail];
        slot.len = data.len() as u16;
        slot.data[..data.len()].copy_from_slice(data);
        self.tail = (self.tail + 1) % RING_CAPACITY;
        self.count += 1;
        Ok(())
    }

    /// Pops the oldest queued packet into `out`, truncating to `out.len()`
    /// if the packet is larger. Returns the number of bytes copied, or
    /// `None` if the ring is empty.
    fn try_pop(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let slot = &self.slots[self.head];
        let n = (slot.len as usize).min(out.len());
        out[..n].copy_from_slice(&slot.data[..n]);
        self.head = (self.head + 1) % RING_CAPACITY;
        self.count -= 1;
        Some(n)
    }
}

/// One registered driver: its name, the packed task id serving it, and its
/// TX/RX packet channel.
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

    /// App → Driver packets, drained by the driver's own `NetRecv`.
    tx_ring: PacketRing,

    /// Driver → App packets, drained by an app's `NetRecv`.
    rx_ring: PacketRing,
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
    // Both rings start empty.
    let mut name_buf = [0u8; DRIVER_NAME_LEN];
    name_buf[..name.len()].copy_from_slice(name);
    registry.push(DriverEntry {
        name: name_buf,
        name_len: name.len(),
        tid,
        tx_ring: PacketRing::new(),
        rx_ring: PacketRing::new(),
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
/// Closure-based so callers (packet rings, status snapshots) can attach data
/// to an entry under the same lock acquisition, without this module exposing
/// a raw guard type that would let a caller hold the lock across unrelated
/// work.
#[allow(dead_code)] // Also consumed by a later Phase-2 step (DrvQuery).
pub fn with_entry_mut<R>(tid: usize, f: impl FnOnce(&mut DriverEntry) -> R) -> Option<R> {
    DRIVER_REGISTRY
        .lock()
        .iter_mut()
        .find(|entry| entry.tid == tid)
        .map(f)
}

/// Pushes `data` into `driver_id`'s channel.
///
/// `caller_is_driver` must be `true` only when the calling task's own packed
/// id equals `driver_id` — see the role-based direction rule documented on
/// the `NetSend` syscall in `kernel/src/syscall/dispatch/driver.rs`:
/// - `true`  (the driver pushing to its own channel): targets the RX ring
///   (Driver → App).
/// - `false` (an app pushing to a driver's channel): targets the TX ring
///   (App → Driver).
///
/// Fails with `InvalidArg` if no entry is registered under `driver_id` (the
/// driver may never have registered, or may have already exited — see
/// [`release_task`]).
pub fn push_packet(
    driver_id: usize,
    caller_is_driver: bool,
    data: &[u8],
) -> Result<(), SyscallError> {
    let outcome = with_entry_mut(driver_id, |entry| {
        let ring = if caller_is_driver {
            &mut entry.rx_ring
        } else {
            &mut entry.tx_ring
        };
        ring.push(data)
    });
    outcome.unwrap_or(Err(SyscallError::InvalidArg))
}

/// Pops the oldest queued packet from `driver_id`'s channel into `out`,
/// mirroring [`push_packet`]'s role-based ring selection (the *opposite*
/// ring from what `push_packet` would touch for the same `caller_is_driver`).
///
/// Returns `Ok(None)` if the ring is empty, or `Err(InvalidArg)` if no entry
/// is registered under `driver_id`.
pub fn try_pop_packet(
    driver_id: usize,
    caller_is_driver: bool,
    out: &mut [u8],
) -> Result<Option<usize>, SyscallError> {
    let outcome = with_entry_mut(driver_id, |entry| {
        let ring = if caller_is_driver {
            &mut entry.tx_ring
        } else {
            &mut entry.rx_ring
        };
        ring.try_pop(out)
    });
    outcome.ok_or(SyscallError::InvalidArg)
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
