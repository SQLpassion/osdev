//! The `drivers` app's `load <name.drv>` command (Phase 2 Step 7 of
//! `docs/nic_driver_design.md` §4.2-4.3): spawns a NIC driver as a
//! background process, without blocking on `process::wait`.
//!
//! Moved here from the shell (`user_programs/shell/src/load_driver.rs`,
//! issue #105) — `load` used to work only because it ran inside the
//! shell's own already-privileged process; `spawn_driver` below requires
//! `Capabilities::SPAWN_DRIVER`, delegated to `DRIVERS.BIN` by the shell at
//! `Exec` time (issue #107). Until that delegation is wired up, `load`
//! fails with `PermissionDenied` — a known, documented intermediate state.
//!
//! PCI-ID-to-driver-name matching is split into a pure, I/O-free function
//! (`resolve_driver_filename`) so it can be unit tested directly, per this
//! project's convention (see `user_programs/rtl8139/src/tests/rtl8139.rs`)
//! of never host-testing the syscall-touching code around such logic.

#[cfg(not(test))]
use lib_kaos::{pci, println};

/// Maps a driver's FAT32 8.3 filename to the PCI `(vendor_id, device_id)`
/// pairs it can service. Mirrors `DRIVER_TABLE` from
/// `docs/nic_driver_design.md` §4.3.
///
/// Source of truth for these PCI IDs: `kernel/src/drivers/driver_db.rs`'s
/// own `RTL8139_IDS`/`INTEL_NIC_IDS` tables, which is what `SpawnDriver`
/// actually derives grants from -- this table only decides which PCI
/// devices are *worth attempting* to load a given driver for, and prints a
/// friendly "no device present" error otherwise; it never grants resources
/// itself (see `load_driver`'s doc comment).
pub const DRIVER_TABLE: &[(&str, u16, u16)] = &[
    ("RTL8139.DRV", 0x10EC, 0x8139),
    ("INTLNIC.DRV", 0x8086, 0x10EA),
    ("INTLNIC.DRV", 0x8086, 0x15B8),
    ("INTLNIC.DRV", 0x8086, 0x10D3),
    ("INTLNIC.DRV", 0x8086, 0x100E),
];

/// Why `resolve_driver_filename` could not resolve `file` to a loadable driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// `file` does not match any entry in the driver table at all.
    UnknownDriver,
    /// `file` matches a table entry, but none of its PCI IDs are present in
    /// the given device list.
    DeviceNotPresent,
}

/// Resolves `file` (matched case-insensitively, per the 8.3 filename
/// convention) against `table`, succeeding only if at least one of its PCI
/// ID entries is present in `discovered_devices`.
///
/// Returns the table's canonical filename spelling (not `file` itself), so
/// the caller always spawns using the exact casing `kernel::drivers::
/// driver_db::DRIVER_DB` expects (that lookup is itself case-insensitive
/// too, but keeping one canonical spelling here avoids relying on that).
pub fn resolve_driver_filename<'t>(
    file: &str,
    table: &'t [(&'t str, u16, u16)],
    discovered_devices: &[(u16, u16)],
) -> Result<&'t str, ResolveError> {
    // Step 1: is `file` a known driver name at all? Checked separately from
    // the PCI-presence scan below so "unknown driver" and "device not
    // present" are distinguishable error messages.
    let mut known = false;

    for &(name, vendor_id, device_id) in table {
        if !name.eq_ignore_ascii_case(file) {
            continue;
        }
        known = true;

        // Step 2: does this entry's PCI ID match a discovered device?
        if discovered_devices.contains(&(vendor_id, device_id)) {
            return Ok(name);
        }
    }

    if known {
        Err(ResolveError::DeviceNotPresent)
    } else {
        Err(ResolveError::UnknownDriver)
    }
}

/// Scans the PCI bus and returns every discovered device's `(vendor_id,
/// device_id)` pair.
#[cfg(not(test))]
fn discovered_pci_ids() -> alloc::vec::Vec<(u16, u16)> {
    let dev_count = pci::get_pci_device_count().unwrap_or(0);
    let mut ids = alloc::vec::Vec::new();

    for i in 0..dev_count {
        let mut dev = pci::UserPciDevice {
            bus: 0,
            device: 0,
            function: 0,
            class_code: 0,
            subclass: 0,
            prog_if: 0,
            revision_id: 0,
            header_type: 0,
            vendor_id: 0,
            device_id: 0,
            interrupt_line: 0,
            interrupt_pin: 0,
            _padding: [0; 2],
            bars: [pci::UserPciBar {
                bar_type: 0,
                flags: 0,
                address: 0,
                size: 0,
                raw_value: 0,
                _padding: 0,
            }; 6],
        };
        if pci::get_pci_device(i, &mut dev).is_ok() {
            ids.push((dev.vendor_id, dev.device_id));
        }
    }

    ids
}

/// Spawns `file` as a background driver process (no `process::wait` --
/// the driver runs independently and the REPL prompt returns immediately).
///
/// Resource grants (MMIO regions, IRQ vector) are **not** computed here and
/// `None` is passed to `spawn_driver`: the kernel always derives the
/// authoritative grant itself from its own PCI enumeration
/// (`kernel::drivers::driver_db::derive_grants`, called from
/// `syscall_spawn_driver_impl`) precisely so an unprivileged caller can
/// never hand itself an arbitrary physical-memory grant by lying about it.
/// `None` means "accept the kernel-derived grant unconditionally", which is
/// exactly what every existing driver-spawn path in this codebase already
/// relies on.
#[cfg(not(test))]
pub fn load_driver(file: &str) {
    // Step 1+2+3: resolve the filename against the driver table and the
    // live PCI bus; print a clear, distinct error for each failure mode
    // without spawning anything.
    let devices = discovered_pci_ids();
    let canonical_name = match resolve_driver_filename(file, DRIVER_TABLE, &devices) {
        Ok(name) => name,
        Err(ResolveError::UnknownDriver) => {
            println!("[drivers] Unknown driver '{}'.", file);
            return;
        }
        Err(ResolveError::DeviceNotPresent) => {
            println!(
                "[drivers] Error: no matching PCI device found for '{}'.",
                file
            );
            return;
        }
    };

    // Step 4: grants are derived kernel-side (see doc comment above); pass
    // None to accept them unconditionally.
    let caps = 1; // MMIO (1)

    // Step 5: spawn in the background -- no process::wait() call.
    match lib_driver::spawn::spawn_driver(canonical_name, caps, None) {
        Ok(tid) => {
            println!(
                "[drivers] Driver '{}' started as TID {}",
                canonical_name, tid
            );
        }
        Err(err) => {
            println!("[drivers] Failed to load '{}': {:?}", canonical_name, err);
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/load_driver.rs"]
mod tests;
