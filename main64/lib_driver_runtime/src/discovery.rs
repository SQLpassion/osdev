//! PCI device discovery and MMIO BAR mapping shared by every NIC driver.
//!
//! Split into pure decision logic (testable without touching hardware) and
//! the syscall calls that supply its inputs, per the project's convention
//! (see `user_programs/rtl8139/src/tests/rtl8139.rs`, which only tests
//! hardware-independent helpers, never syscall-touching code).

use lib_kaos::pci::{self, UserPciBar, UserPciDevice};

/// One entry in a driver's PCI-match table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciMatch {
    pub vendor_id: u16,
    pub device_id: u16,
}

/// Returns the index of the first entry in `table` whose `(vendor_id,
/// device_id)` matches `dev`, if any. Pure decision logic, no I/O.
pub fn find_matching_index(table: &[PciMatch], dev: &UserPciDevice) -> Option<usize> {
    table
        .iter()
        .position(|m| m.vendor_id == dev.vendor_id && m.device_id == dev.device_id)
}

/// Scans the PCI bus for the first device matching any entry in `table`.
///
/// Returns the device and the index of the `table` entry it matched, so
/// callers that need model-specific data keyed by table position (e.g. the
/// Intel driver's `NicModel`) can look it up without a second match.
pub fn find_pci_device(table: &[PciMatch]) -> Option<(UserPciDevice, usize)> {
    // Step 1: enumerate every PCI device the kernel has cached.
    let dev_count = pci::get_pci_device_count().unwrap_or(0);

    // Step 2: an all-zero UserPciDevice/UserPciBar as the read destination
    // for each iteration -- mirrors the struct literal both driver main.rs
    // files used to construct inline before this extraction.
    let zero_bar = UserPciBar {
        bar_type: 0,
        flags: 0,
        address: 0,
        size: 0,
        raw_value: 0,
        _padding: 0,
    };
    let mut dev = UserPciDevice {
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
        bars: [zero_bar; 6],
    };

    // Step 3: check each enumerated device against the match table in PCI
    // enumeration order, returning the first hit.
    for i in 0..dev_count {
        if pci::get_pci_device(i, &mut dev).is_ok() {
            if let Some(idx) = find_matching_index(table, &dev) {
                return Some((dev, idx));
            }
        }
    }

    None
}

/// Selects which BAR index to map: the first Memory-type BAR (type 2 or 3)
/// with a non-zero address, or `preferred_index` if none matches and that
/// index is itself a non-zero BAR. Pure decision logic, no I/O.
///
/// A size-0 BAR cannot be granted an MMIO window by the kernel (see
/// `kernel::drivers::driver_db::mmio_windows`, which skips unsizable BARs
/// rather than fabricating a length) -- this function only checks `address`;
/// callers must still reject a selected BAR whose `size` is 0.
pub fn select_mmio_bar_index(
    bars: &[UserPciBar; 6],
    preferred_index: Option<usize>,
) -> Option<usize> {
    if let Some(idx) = bars
        .iter()
        .position(|bar| (bar.bar_type == 2 || bar.bar_type == 3) && bar.address != 0)
    {
        return Some(idx);
    }

    let idx = preferred_index?;
    if bars[idx].address != 0 {
        Some(idx)
    } else {
        None
    }
}

/// Failure reason for [`map_mmio_bar`].
#[derive(Debug)]
pub enum MmioMapError {
    /// No BAR matched `select_mmio_bar_index`, or the selected BAR reports
    /// a zero size (unsizable, not grantable by the kernel).
    NoUsableBar,
    /// The selected BAR was valid, but `Mmio::map` itself failed.
    Map(lib_driver::SysError),
}

/// Selects and maps a device's MMIO BAR into this task's address space.
///
/// `preferred_index` is used only as a fallback if no BAR is itself a valid
/// Memory-type BAR with a non-zero address -- see [`select_mmio_bar_index`].
pub fn map_mmio_bar(
    dev: &UserPciDevice,
    preferred_index: Option<usize>,
) -> Result<lib_driver::mmio::Mmio, MmioMapError> {
    let idx = select_mmio_bar_index(&dev.bars, preferred_index).ok_or(MmioMapError::NoUsableBar)?;
    let bar = dev.bars[idx];
    if bar.size == 0 {
        return Err(MmioMapError::NoUsableBar);
    }
    lib_driver::mmio::Mmio::map(bar.address, bar.size as usize).map_err(MmioMapError::Map)
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/discovery.rs"]
mod tests;
