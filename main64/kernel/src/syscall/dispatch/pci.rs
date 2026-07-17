//! PCI-related system call implementations.

use crate::drivers::pci;
use crate::syscall::types::{
    is_valid_user_buffer_writable, SyscallError, SyscallResult, UserPciBar, UserPciDevice,
};

/// Implements `GetPciDeviceCount()`.
///
/// Returns the total count of discovered PCI devices on the bus scan.
pub fn syscall_get_pci_device_count_impl() -> SyscallResult<u64> {
    // Step 1: Query the boot-time cached PCI device registry.
    let count = pci::get_devices().len();
    Ok(count as u64)
}

/// Implements `GetPciDevice()`.
///
/// Copies metadata of a specific PCI device into user space.
pub fn syscall_get_pci_device_impl(index: u64, out_ptr: *mut UserPciDevice) -> SyscallResult<u64> {
    // Step 1: Query the single cached PCI device directly to avoid cloning the entire vector.
    let dev = match pci::get_device(index as usize) {
        Some(d) => d,
        None => return Err(SyscallError::InvalidArg),
    };

    // Step 2: Verify that the user-space output pointer represents a valid,
    // writable memory range in the Ring-3 address space.
    let struct_size = core::mem::size_of::<UserPciDevice>();
    if !is_valid_user_buffer_writable(out_ptr as *const u8, struct_size) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: Map and construct the user-compatible BAR structures.
    let mut bars = [UserPciBar {
        bar_type: 0,
        flags: 0,
        address: 0,
        size: 0,
        raw_value: 0,
        _padding: 0,
    }; 6];

    for (i, raw_bar) in dev.bars.iter().enumerate() {
        let (bar_type, address, size, prefetchable) = match raw_bar.bar_type {
            pci::BarType::None => (0, 0, 0, false),
            pci::BarType::Io { port, size } => (1, port as u64, size as u64, false),
            pci::BarType::Memory32 {
                address,
                size,
                prefetchable,
            } => (2, address as u64, size as u64, prefetchable),
            pci::BarType::Memory64 {
                address,
                size,
                prefetchable,
            } => (3, address, size, prefetchable),
        };
        bars[i] = UserPciBar {
            bar_type,
            flags: if prefetchable { 1 } else { 0 },
            address,
            size,
            raw_value: raw_bar.raw_value,
            _padding: 0,
        };
    }

    let user_dev = UserPciDevice {
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
        _padding: [0; 2],
        bars,
    };

    // SAFETY:
    // - `out_ptr` has been validated to point entirely within present,
    //   user-accessible, writable pages.
    // - The memory alignment is handled by `UserPciDevice` being `#[repr(C)]`.
    // - Memory safety is preserved since the caller owns the memory range in user space.
    unsafe {
        out_ptr.write(user_dev);
    }

    Ok(0)
}
