//! BIOS memory map query wrappers for Ring-3 programs.

use crate::{
    decode_result,
    raw::{syscall0, syscall2},
    SysError, SyscallId,
};

pub use crate::kernel_types::UserBiosMemoryRegion;

/// Queries the total count of BIOS memory map entries populated by the bootloader.
#[inline(always)]
pub fn get_bios_memory_map_entry_count() -> Result<usize, SysError> {
    let raw = unsafe {
        // SAFETY: `GetBiosMemoryMapEntryCount` takes no arguments and has no memory hazards.
        syscall0(SyscallId::GetBiosMemoryMapEntryCount as u64)
    };

    decode_result(raw).map(|count| count as usize)
}

/// Copies the BIOS memory map entry metadata at the given index into the output buffer.
#[inline(always)]
pub fn get_bios_memory_map_entry(
    index: usize,
    out: &mut UserBiosMemoryRegion,
) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `out` is a valid mutable reference whose address and size are safe to be written by the kernel.
        syscall2(
            SyscallId::GetBiosMemoryMapEntry as u64,
            index as u64,
            out as *mut UserBiosMemoryRegion as u64,
        )
    };

    decode_result(raw).map(|_| ())
}
