//! Kernel-side driver database and authoritative resource-grant derivation.
//!
//! Design summary:
//! - `SpawnDriver` must never accept resource grants from user space. A caller that
//!   could name its own physical region would be able to map arbitrary physical
//!   memory — including kernel frames — into a Ring-3 address space via
//!   `MapPhysical`, defeating the isolation the capability system exists to provide.
//! - The kernel therefore derives every grant itself from its own PCI enumeration.
//!   The driver binary's name selects *which* device the driver may bind to; it never
//!   selects the address range.
//! - This is the binding step described in `docs/todo_drivers.md` §6 ("grants are
//!   derived from the device's PCI data"), moved from the user-space caller into the
//!   kernel where it is trustworthy.
//!
//! Notes:
//! - Driver binaries live flat in the FAT32 root under 8.3 short names, so names are
//!   compared case-insensitively.
//! - Only memory BARs with a non-zero, successfully sized window are granted. A BAR
//!   that could not be sized is skipped rather than guessed at: a fabricated length
//!   would be exactly the kind of untrusted grant this module removes.

use alloc::vec::Vec;

use crate::arch::constants::PAGE_SIZE_U64;
use crate::drivers::pci::{self, BarType, PciDevice};
use crate::memory::vmm::USER_MMIO_BASE;
use crate::process::capabilities::{Capabilities, ResourceGrants};
use crate::sync::spinlock::SpinLock;

/// Capability flags a spawned driver may ever receive.
///
/// `SPAWN_DRIVER` is deliberately excluded: it belongs to the driver manager alone
/// and must not be propagated into a spawned driver, which would let any driver
/// mint further drivers with capabilities of its own choosing.
pub const DRIVER_GRANTABLE_CAPS: Capabilities = Capabilities::MMIO.union(Capabilities::IRQ);

/// PCI IDs the Realtek RTL8139 Fast Ethernet driver may bind to.
const RTL8139_IDS: &[(u16, u16)] = &[(0x10EC, 0x8139)];

/// PCI IDs the Intel Gigabit Ethernet driver may bind to:
/// 82577LM, I219-V, 82574L (e1000e) and 82540EM (e1000).
const INTEL_NIC_IDS: &[(u16, u16)] = &[
    (0x8086, 0x10EA),
    (0x8086, 0x15B8),
    (0x8086, 0x10D3),
    (0x8086, 0x100E),
];

/// Maps a driver binary name to the PCI devices that driver is allowed to bind to.
///
/// Adding a driver means adding a row here — the kernel will not hand out a grant for
/// a binary it does not know.
const DRIVER_DB: &[(&str, &[(u16, u16)])] =
    &[("rtl8139.bin", RTL8139_IDS), ("intlnic.bin", INTEL_NIC_IDS)];

/// Why a driver could not be bound to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// The binary name is not a registered driver in [`DRIVER_DB`].
    UnknownDriver,

    /// The driver is registered, but none of its supported devices is present on the PCI bus.
    DeviceNotPresent,

    /// The device is present but exposes no usable (non-zero, sized) memory BAR.
    NoMmioBar,

    /// Every device the driver could bind to is already bound to a live driver task.
    ///
    /// Without this check, two `SpawnDriver` calls for the same binary (a caller
    /// bug, a race between two callers, or a respawn attempted before the first
    /// instance exits) would each derive a grant for the *same* PCI device,
    /// handing two Ring-3 tasks concurrent MMIO/IRQ access to one device's
    /// registers and descriptor rings.
    AllDevicesBound,
}

/// Registry of PCI devices currently bound to a live driver task, keyed by the
/// device's (bus, device, function) triple — the only field combination that
/// uniquely identifies a device when multiple identical cards are installed.
///
/// Entries also transiently hold [`RESERVED_TASK_ID`] between `derive_grants`
/// claiming a device and the caller resolving that claim via
/// [`confirm_binding`] (task created) or [`release_reservation`] (spawn failed
/// after the claim), so the window between "device selected" and "task
/// created" cannot be raced by a second concurrent `SpawnDriver` call for the
/// same device.
static DEVICE_BINDINGS: SpinLock<Vec<(u32, usize)>> = SpinLock::new(Vec::new());

/// Sentinel task ID meaning "device claimed by an in-flight `SpawnDriver` call,
/// task not yet created". Never a legal packed task ID: a real task's
/// generation counter starts at 1, so `usize::MAX` (generation and slot both
/// all-ones) is unreachable by [`pack_task_id`](crate::scheduler::pack_task_id)
/// short of ~4 billion spawns.
const RESERVED_TASK_ID: usize = usize::MAX;

/// Packs a PCI device's (bus, device, function) triple into a lookup key.
fn device_key(device: &PciDevice) -> u32 {
    ((device.bus as u32) << 16) | ((device.device as u32) << 8) | (device.function as u32)
}

/// Unpacks a lookup key produced by [`device_key`] back into (bus, device, function).
fn decode_device_key(key: u32) -> (u8, u8, u8) {
    ((key >> 16) as u8, (key >> 8) as u8, key as u8)
}

/// Atomically claims `device` for an in-flight `SpawnDriver` call.
///
/// Returns `false` without side effects if the device is already bound to a
/// live task or already reserved by another in-flight call — the caller
/// should try the next candidate device rather than hand out a duplicate grant.
fn reserve_device(device: &PciDevice) -> bool {
    let key = device_key(device);
    let mut bindings = DEVICE_BINDINGS.lock();
    if bindings.iter().any(|&(k, _)| k == key) {
        return false;
    }
    bindings.push((key, RESERVED_TASK_ID));
    true
}

/// Resolves a device reservation to the task that was actually spawned.
///
/// Called once `SpawnDriver` has successfully created the driver task, so the
/// binding's lifetime from here on matches the lifetime of that task's
/// `DriverCaps` block (freed in `remove_task`, see [`release_task`]).
pub fn confirm_binding(device: &PciDevice, task_id: usize) {
    let key = device_key(device);
    let mut bindings = DEVICE_BINDINGS.lock();
    if let Some(entry) = bindings
        .iter_mut()
        .find(|(k, t)| *k == key && *t == RESERVED_TASK_ID)
    {
        entry.1 = task_id;
    }
}

/// Releases a device reservation that was never confirmed because `SpawnDriver`
/// failed after `derive_grants` claimed the device (e.g. the requested grant
/// was rejected, or the driver binary could not be loaded).
///
/// Also disables the device's I/O/Memory/Bus-Master decode bits that
/// `derive_grants`'s Step 4b enabled when it reserved this device. Without
/// this, a spawn failure after that point would leave the device permanently
/// decoding MMIO and mastering DMA with no owning task — contradicting this
/// module's own invariant that an ungranted device never decodes MMIO or
/// masters DMA.
pub fn release_reservation(device: &PciDevice) {
    let key = device_key(device);
    let mut bindings = DEVICE_BINDINGS.lock();
    bindings.retain(|&(k, t)| !(k == key && t == RESERVED_TASK_ID));
    drop(bindings);

    pci::disable_device(device);
}

/// Releases every device binding owned by `task_id`.
///
/// Called from the scheduler's `remove_task` — the single choke point reached
/// by both explicit termination and zombie-reaping after a crash — mirroring
/// `irq_bridge::release_task`. Without this, a device bound to a crashed or
/// exited driver task would stay bound forever, permanently refusing
/// `SpawnDriver` for that device with `AllDevicesBound`.
///
/// Also disables every released device's I/O/Memory/Bus-Master decode bits
/// before returning. `remove_task` calls this before it defers the task's
/// address space (and any DMA buffers still targeted by a live descriptor
/// ring) for teardown, so a NIC with armed RX/TX descriptors can no longer
/// perform DMA by the time those physical frames are handed back to the PMM
/// and reused by an unrelated task.
pub fn release_task(task_id: usize) {
    if task_id == 0 {
        return;
    }
    let mut bindings = DEVICE_BINDINGS.lock();
    let released_keys: Vec<u32> = bindings
        .iter()
        .filter(|&&(_, t)| t == task_id)
        .map(|&(k, _)| k)
        .collect();
    bindings.retain(|&(_, t)| t != task_id);
    drop(bindings);

    if released_keys.is_empty() {
        return;
    }
    let devices = pci::get_devices();
    for key in released_keys {
        let (bus, device, function) = decode_device_key(key);
        if let Some(dev) = devices
            .iter()
            .find(|d| d.bus == bus && d.device == device && d.function == function)
        {
            pci::disable_device(dev);
        }
    }
}

/// Resets all device bindings (for unit tests / teardown).
pub fn reset_bindings_for_test() {
    DEVICE_BINDINGS.lock().clear();
}

/// Exercises [`reserve_device`] directly (for unit tests).
///
/// The QEMU configuration used by the integration test runner attaches no
/// PCI NIC, so `derive_grants` can never reach a live device to exercise the
/// double-grant check end to end. This lets tests drive the same reservation
/// logic with a synthetic [`PciDevice`] instead.
pub fn reserve_device_for_test(device: &PciDevice) -> bool {
    reserve_device(device)
}

/// Exercises [`mmio_windows`] directly (for unit tests).
///
/// `derive_grants` can only reach this indirectly through a live PCI device,
/// which the integration test runner's QEMU configuration never attaches
/// (see [`reserve_device_for_test`]). This lets tests drive the BAR-to-window
/// derivation itself, including malformed/adversarial `BarType::Memory64`
/// values a real device could never present but a misbehaving VM or a
/// spoofed config-space read could.
pub fn mmio_windows_for_test(device: &PciDevice) -> Vec<(u64, u64)> {
    mmio_windows(device)
}

/// Restricts a caller-supplied capability bitmask to the flags a driver may hold.
///
/// Unknown bits are dropped by `from_bits_truncate`; `SPAWN_DRIVER` is then masked
/// off by [`DRIVER_GRANTABLE_CAPS`].
pub fn sanitize_driver_caps(caps_flags: u64) -> Capabilities {
    Capabilities::from_bits_truncate(caps_flags as u32) & DRIVER_GRANTABLE_CAPS
}

/// Looks up the (vendor, device) pairs a driver binary is allowed to bind to.
pub fn lookup_driver(name: &str) -> Option<&'static [(u16, u16)]> {
    DRIVER_DB
        .iter()
        .find(|(db_name, _)| db_name.eq_ignore_ascii_case(name))
        .map(|(_, ids)| *ids)
}

/// Returns true if `name` is a registered driver binary.
pub fn is_known_driver(name: &str) -> bool {
    lookup_driver(name).is_some()
}

/// Extracts the usable memory BAR windows of a device as `(phys_base, len_bytes)` pairs.
///
/// I/O BARs are ignored: KAOS drivers reach port-mapped registers through the
/// mediated syscall path, not through an MMIO grant.
///
/// Each window is rounded out to whole 4KiB pages. `MapPhysical` can only map
/// at page granularity, so a BAR smaller than or unaligned to a page (PCI
/// legally allows BARs as small as 16 bytes) always ends up exposing its
/// entire enclosing page(s) once mapped. Storing the grant pre-rounded makes
/// it the authoritative statement of what the driver actually gets access
/// to, instead of a narrower byte range that `MapPhysical`'s own
/// page-rounded grant check would then have to reject as out-of-bounds.
fn mmio_windows(device: &PciDevice) -> Vec<(u64, u64)> {
    let mut windows = Vec::new();

    for bar in &device.bars {
        let window = match bar.bar_type {
            BarType::Memory32 { address, size, .. } => (address as u64, size as u64),
            BarType::Memory64 { address, size, .. } => (address, size),
            BarType::None | BarType::Io { .. } => continue,
        };

        // Skip unmapped or unsizable BARs — see the module note on fabricated lengths.
        if window.0 == 0 || window.1 == 0 {
            continue;
        }

        // Skip a BAR whose page-aligned window would overflow `u64` rather
        // than let it wrap into a corrupted grant: a `Memory64` BAR's
        // (address, size) pair comes straight from hardware/emulator PCI
        // config-space registers, so a device (or a malicious/buggy VM) can
        // present values close to `u64::MAX` here. Silently wrapping would
        // hand `MapPhysical` a `page_base..page_end` range with `page_end <
        // page_base`, whose `page_end - page_base` subtraction below would
        // itself underflow into a huge bogus length — exactly the kind of
        // fabricated grant this module's module-level doc note warns against.
        let page_base = window.0 & !(PAGE_SIZE_U64 - 1);
        let Some(raw_end) = window.0.checked_add(window.1) else {
            continue;
        };
        let Some(page_end) = raw_end.checked_add(PAGE_SIZE_U64 - 1) else {
            continue;
        };
        let page_end = page_end & !(PAGE_SIZE_U64 - 1);
        windows.push((page_base, page_end - page_base));
    }

    windows
}

/// Derives the authoritative resource grants for a driver binary from live PCI data.
///
/// Returns the grants together with the PCI device the driver was bound to, so the
/// caller can log the binding and cross-check what the requester asked for.
///
/// The returned device is atomically reserved (see [`reserve_device`]) before this
/// function returns `Ok`, so two concurrent calls for the same driver can never be
/// handed the same device. The caller must resolve that reservation by calling
/// exactly one of [`confirm_binding`] (task successfully spawned) or
/// [`release_reservation`] (spawn failed afterwards) for the returned device.
pub fn derive_grants(name: &str) -> Result<(ResourceGrants, PciDevice), BindError> {
    // Step 1: Resolve the binary name to the set of devices this driver may bind to.
    let supported = lookup_driver(name).ok_or(BindError::UnknownDriver)?;

    // Step 2: Walk enumerated devices matching one of those IDs, in PCI enumeration
    // order, skipping any device already bound (or reserved) by another driver task.
    // This is what prevents two `SpawnDriver` calls for the same binary — a caller
    // bug, a race, or a respawn attempted before the first instance exits — from
    // both being handed the same device's MMIO/IRQ grant.
    let devices = pci::get_devices();
    let mut device_present = false;
    for device in devices.iter().filter(|dev| {
        supported
            .iter()
            .any(|&(vendor, device_id)| dev.vendor_id == vendor && dev.device_id == device_id)
    }) {
        device_present = true;

        // Step 3: Derive the MMIO regions from this device's own BARs. A device
        // without a usable BAR is a hard failure rather than a skip-and-retry
        // candidate: it is not a resource contention problem, so trying the next
        // matching device (if any) would not help.
        let mmio_regions = mmio_windows(device);
        if mmio_regions.is_empty() {
            return Err(BindError::NoMmioBar);
        }

        // Step 4: Atomically claim this device. If it is already bound (or
        // reserved by a concurrent in-flight SpawnDriver), try the next
        // matching device instead of failing outright.
        if !reserve_device(device) {
            continue;
        }

        // Step 4b: Only now — once this specific device is exclusively reserved
        // for the driver being spawned — enable its Command Register bits. PCI
        // enumeration deliberately leaves every device's I/O/Memory/Bus-Master
        // bits untouched so an ungranted device never decodes MMIO or masters DMA.
        pci::enable_device(device);

        // Step 5: Derive the IRQ grant from the device's interrupt line. 0xFF
        // means "no interrupt routed", which is not a grantable vector.
        let mut irqs = Vec::new();
        if device.interrupt_line != 0xFF {
            irqs.push(device.interrupt_line);
        }

        return Ok((
            ResourceGrants {
                mmio_regions,
                irqs,
                mmio_bump: USER_MMIO_BASE,
            },
            *device,
        ));
    }

    if !device_present {
        return Err(BindError::DeviceNotPresent);
    }

    Err(BindError::AllDevicesBound)
}

/// Checks that a caller's *requested* grant is consistent with the kernel-derived one.
///
/// The kernel does not adopt the requested values — [`derive_grants`] is authoritative.
/// This check exists so that a caller asking for a region or vector that does not
/// belong to its device is rejected loudly instead of silently downgraded.
///
/// A request of `0` (MMIO base) or `0xFF` (IRQ) means "no preference" and always passes.
pub fn request_matches_grants(
    grants: &ResourceGrants,
    requested_mmio_base: u64,
    requested_irq: u8,
) -> bool {
    if requested_mmio_base != 0 {
        let inside_grant = grants.mmio_regions.iter().any(|&(base, len)| {
            base.checked_add(len)
                .is_some_and(|end| requested_mmio_base >= base && requested_mmio_base < end)
        });
        if !inside_grant {
            return false;
        }
    }

    if requested_irq != 0xFF && !grants.irqs.contains(&requested_irq) {
        return false;
    }

    true
}
