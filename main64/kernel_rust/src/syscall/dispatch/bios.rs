//! BIOS-related system call implementations.

use crate::memory::bios::{self, BiosInformationBlock, BiosMemoryRegion};
use crate::syscall::types::{is_valid_user_buffer, SyscallError, SyscallResult, UserBiosMemoryRegion};

/// Implements `GetBiosMemoryMapEntryCount()`.
///
/// Returns the total count of BIOS memory map entries populated by the bootloader.
pub fn syscall_get_bios_memory_map_entry_count_impl() -> SyscallResult<u64> {
    // SAFETY:
    // - The bootloader has populated the BIOS Information Block at `BIB_OFFSET`.
    // - The memory is read-only from the kernel's perspective, representing static system information.
    // - There are no concurrent writers, ensuring memory safety.
    let bib = unsafe { &*(bios::BIB_OFFSET as *const BiosInformationBlock) };
    Ok(bib.memory_map_entries as u64)
}

/// Implements `GetBiosMemoryMapEntry()`.
///
/// Copies metadata of a specific BIOS memory map entry into user space.
pub fn syscall_get_bios_memory_map_entry_impl(
    index: u64,
    out_ptr: *mut UserBiosMemoryRegion,
) -> SyscallResult<u64> {
    // SAFETY:
    // - The bootloader has populated the BIOS Information Block at `BIB_OFFSET`.
    // - The memory is read-only and static, preventing data races.
    let bib = unsafe { &*(bios::BIB_OFFSET as *const BiosInformationBlock) };

    // Step 1: Validate that the index is within the bounds of populated BIOS memory regions.
    if index >= bib.memory_map_entries as u64 {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Verify that the user-space output pointer represents a valid,
    // writable memory range in the Ring-3 address space.
    let struct_size = core::mem::size_of::<UserBiosMemoryRegion>();
    if !is_valid_user_buffer(out_ptr as *const u8, struct_size) {
        return Err(SyscallError::InvalidArg);
    }

    let region = bios::MEMORYMAP_OFFSET as *const BiosMemoryRegion;

    // SAFETY:
    // - `index` has been validated to be less than the total entries.
    // - `region` points to a contiguous array of `BiosMemoryRegion` structures at `MEMORYMAP_OFFSET`.
    // - Out-of-bounds pointer arithmetic is prevented by the validation check.
    let current_region = unsafe { &*region.add(index as usize) };

    let user_region = UserBiosMemoryRegion {
        start: current_region.start,
        size: current_region.size,
        region_type: current_region.region_type,
        _padding: 0,
    };

    // SAFETY:
    // - `out_ptr` has been validated to point entirely within user canonical space.
    // - The memory alignment is handled by `UserBiosMemoryRegion` being `#[repr(C)]`.
    // - Memory safety is preserved since the caller owns the memory range in user space.
    unsafe {
        out_ptr.write(user_region);
    }

    Ok(0)
}

/// Implements `GetTime()`.
///
/// Copies the current high-precision calendar date and time into the user-space output pointer.
pub fn syscall_get_time_impl(
    out_ptr: *mut crate::syscall::types::UserDateTime,
) -> SyscallResult<u64> {
    // Step 1: Verify that the user-space output pointer represents a valid,
    // writable memory range in the Ring-3 address space.
    let struct_size = core::mem::size_of::<crate::syscall::types::UserDateTime>();
    if !is_valid_user_buffer(out_ptr as *const u8, struct_size) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Query the high-precision system time from the time driver.
    let current = crate::drivers::time::get_time();

    let user_dt = crate::syscall::types::UserDateTime {
        year: current.year,
        month: current.month,
        day: current.day,
        hour: current.hour,
        minute: current.minute,
        second: current.second,
        _padding: [0; 7],
    };

    // SAFETY:
    // - `out_ptr` has been validated to point entirely within user canonical space.
    // - The memory alignment is handled by `UserDateTime` being `#[repr(C)]`.
    // - Memory safety is preserved since the caller owns the memory range in user space.
    unsafe {
        out_ptr.write(user_dt);
    }

    Ok(0)
}

