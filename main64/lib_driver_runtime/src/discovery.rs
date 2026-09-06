//! PCI device discovery and MMIO BAR mapping shared by every NIC driver.
//!
//! Split into pure decision logic (testable without touching hardware) and
//! the syscall calls that supply its inputs, per the project's convention
//! (see `user_programs/rtl8139/src/tests/rtl8139.rs`, which only tests
//! hardware-independent helpers, never syscall-touching code).

use lib_kaos::pci::{UserPciBar, UserPciDevice};

/// Converts `lib_driver`'s ABI mirror of `UserPciDevice` into this crate's
/// own. Both are independent `#[path]` imports of the same kernel struct
/// definition (`kernel/src/syscall/types.rs`), so Rust treats them as
/// distinct, non-interchangeable types despite identical layout — this is
/// the one place that bridges them.
fn from_lib_driver_device(dev: lib_driver::UserPciDevice) -> UserPciDevice {
    let bars = dev.bars.map(|bar| UserPciBar {
        bar_type: bar.bar_type,
        flags: bar.flags,
        address: bar.address,
        size: bar.size,
        raw_value: bar.raw_value,
        _padding: bar._padding,
    });
    UserPciDevice {
        bus: dev.bus,
        device: dev.device,
        function: dev.function,
        class_code: dev.class_code,
        subclass: dev.subclass,
        prog_if: dev.prog_if,
        revision_id: dev.revision_id,
        header_type: dev.header_type,
        vendor_id: dev.vendor_id,
        device_id: dev.device_id,
        interrupt_line: dev.interrupt_line,
        interrupt_pin: dev.interrupt_pin,
        _padding: dev._padding,
        bars,
    }
}

/// Returns the PCI device the kernel bound this driver task to at
/// `SpawnDriver` time, via `lib_driver::spawn::bound_device`.
///
/// Replaces the driver-side PCI bus scan and vendor/device-ID table this
/// crate used to require (`find_pci_device`/`PciMatch`, removed): the
/// kernel's own `driver_db::DRIVER_DB` already validated which device this
/// driver binary may bind to, and already selected the exact device at
/// `SpawnDriver` time (`derive_grants`) — re-deriving that decision
/// independently here was two sources of truth for the same mapping, and
/// could not be relied on to agree with the kernel's choice if more than one
/// matching card were installed.
pub fn find_bound_device() -> Option<UserPciDevice> {
    lib_driver::spawn::bound_device()
        .ok()
        .map(from_lib_driver_device)
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
