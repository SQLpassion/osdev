//! PCI bus query wrappers for Ring-3 programs.

use crate::{
    decode_result,
    raw::{syscall0, syscall2},
    SysError, SyscallId,
};

pub use crate::kernel_types::{UserPciBar, UserPciDevice};

/// Queries the total count of discovered PCI devices on the system.
#[inline(always)]
pub fn get_pci_device_count() -> Result<usize, SysError> {
    let raw = unsafe {
        // SAFETY: `GetPciDeviceCount` takes no arguments and has no memory hazards.
        syscall0(SyscallId::GetPciDeviceCount as u64)
    };

    decode_result(raw).map(|count| count as usize)
}

/// Copies the PCI device information at the given index into the output buffer.
#[inline(always)]
pub fn get_pci_device(index: usize, out: &mut UserPciDevice) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `out` is a valid mutable reference whose address and size are safe to be written by the kernel.
        syscall2(
            SyscallId::GetPciDevice as u64,
            index as u64,
            out as *mut UserPciDevice as u64,
        )
    };

    decode_result(raw).map(|_| ())
}
