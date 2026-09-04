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

use crate::drivers::pci::{self, BarType, PciDevice};
use crate::memory::vmm::USER_MMIO_BASE;
use crate::process::capabilities::{Capabilities, ResourceGrants};

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
const DRIVER_DB: &[(&str, &[(u16, u16)])] = &[
    ("rtl8139.bin", RTL8139_IDS),
    ("intlnic.bin", INTEL_NIC_IDS),
];

/// Why a driver could not be bound to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// The binary name is not a registered driver in [`DRIVER_DB`].
    UnknownDriver,

    /// The driver is registered, but none of its supported devices is present on the PCI bus.
    DeviceNotPresent,

    /// The device is present but exposes no usable (non-zero, sized) memory BAR.
    NoMmioBar,
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

        windows.push(window);
    }

    windows
}

/// Derives the authoritative resource grants for a driver binary from live PCI data.
///
/// Returns the grants together with the PCI device the driver was bound to, so the
/// caller can log the binding and cross-check what the requester asked for.
pub fn derive_grants(name: &str) -> Result<(ResourceGrants, PciDevice), BindError> {
    // Step 1: Resolve the binary name to the set of devices this driver may bind to.
    let supported = lookup_driver(name).ok_or(BindError::UnknownDriver)?;

    // Step 2: Find the first enumerated device matching one of those IDs.
    let devices = pci::get_devices();
    let device = devices
        .iter()
        .find(|dev| {
            supported
                .iter()
                .any(|&(vendor, device_id)| dev.vendor_id == vendor && dev.device_id == device_id)
        })
        .copied()
        .ok_or(BindError::DeviceNotPresent)?;

    // Step 3: Derive the MMIO regions from that device's own BARs.
    let mmio_regions = mmio_windows(&device);
    if mmio_regions.is_empty() {
        return Err(BindError::NoMmioBar);
    }

    // Step 4: Derive the IRQ grant from the device's interrupt line. 0xFF means
    // "no interrupt routed", which is not a grantable vector.
    let mut irqs = Vec::new();
    if device.interrupt_line != 0xFF {
        irqs.push(device.interrupt_line);
    }

    Ok((
        ResourceGrants {
            mmio_regions,
            irqs,
            mmio_bump: USER_MMIO_BASE,
        },
        device,
    ))
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
