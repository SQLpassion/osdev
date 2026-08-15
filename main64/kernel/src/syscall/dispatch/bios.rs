//! BIOS-related system call implementations.

use crate::memory::bios::{self, BiosInformationBlock, BiosMemoryRegion};
use crate::syscall::types::{
    is_valid_user_buffer_writable, SyscallError, SyscallResult, UserBiosMemoryRegion,
};

/// Maps an EFI memory type back to the BIOS/E820 `region_type` convention used by
/// [`UserBiosMemoryRegion`] (1 = usable). This is the inverse of `kaosldr_64`'s
/// `e820_type_to_efi_memory_type`, so a BIOS-boot round-trip is lossless
/// (1→7→1, 2→0→2, 3→9→3, 4→10→4) while UEFI EFI types collapse to the closest
/// E820 class.
fn efi_type_to_e820_region_type(memory_type: u32) -> u32 {
    match memory_type {
        7 => 1,  // EfiConventionalMemory -> Usable
        9 => 3,  // EfiACPIReclaimMemory  -> ACPI reclaimable
        10 => 4, // EfiACPIMemoryNVS      -> ACPI NVS
        _ => 2,  // everything else       -> Reserved
    }
}

/// Returns the loader-published unified memory map as a slice when a `BootInfo` is
/// present — which is the case for both the BIOS and the UEFI loader. Returns `None`
/// only on the legacy no-`BootInfo` path (e.g. bare-metal unit-test kernels), where
/// callers fall back to the fixed low BIOS Information Block.
///
/// This is the boot-path-agnostic source these syscalls must prefer: reading the
/// BIB / memory-map array at the fixed low physical addresses `BIB_OFFSET` (0x1000)
/// and `MEMORYMAP_OFFSET` (0x1200) is only valid on the legacy BIOS path AND assumes
/// that low page is mapped. Under the #63 kernel-owned page tables that page is NOT
/// mapped on UEFI hardware (the firmware marks it a dropped type), so an
/// unconditional read there faults — this is exactly the TUI-startup crash
/// (`page_fault.rs` "protection page fault at 0x100e", i.e. BIB_OFFSET + the offset
/// of `memory_map_entries`) that reading the unified map instead avoids.
fn unified_memory_map() -> Option<&'static [crate::boot_info::UnifiedMemoryEntry]> {
    let bi = crate::boot_info::BootInfo::get()?;
    if bi.memory_map_addr == 0 || bi.memory_map_len == 0 {
        return None;
    }
    // SAFETY: the loader publishes a valid, mapped array of `memory_map_len`
    // `UnifiedMemoryEntry` at `memory_map_addr` (loader-owned memory, covered by the
    // kernel-owned direct map's `is_loader_owned` pass and asserted mapped by
    // `validate_essential_boot_addresses`). Same access pattern as `pmm::manager`
    // and `vmm::direct_map::switch_to_direct_map`.
    Some(unsafe {
        core::slice::from_raw_parts(
            bi.memory_map_addr as *const crate::boot_info::UnifiedMemoryEntry,
            bi.memory_map_len as usize,
        )
    })
}

/// Implements `GetBiosMemoryMapEntryCount()`.
///
/// Returns the total count of memory map entries. Prefers the boot-path-agnostic
/// unified map published by the loader; only falls back to the low BIOS Information
/// Block when no `BootInfo` is present. See [`unified_memory_map`] for why the low
/// read must not be unconditional.
///
/// # Authorization (M10, `docs/CODE_REVIEW_2026-07-26.md`)
/// Like `GetPciDeviceCount`/`GetPciDevice` (`syscall/dispatch/pci.rs`), this
/// syscall is deliberately **not** gated behind `TaskEntry::privileged`. The
/// shipped `TUI.BIN` hardware-inspection screen (`user_programs/tui_app`) is
/// an unprivileged `Exec`-spawned task and relies on this syscall to display
/// the memory map; a privilege gate would break that shipped feature. This
/// single-tenant kernel has no isolation boundary between ring-3 tasks that
/// the E820/UEFI memory map would need to be kept secret across, so the
/// M10 stopgap decision here is "no change" rather than a gate. See
/// `syscall/dispatch/pci.rs`'s `GetPciDeviceCount` doc for the full
/// rationale, which applies identically here.
pub fn syscall_get_bios_memory_map_entry_count_impl() -> SyscallResult<u64> {
    if let Some(map) = unified_memory_map() {
        return Ok(map.len() as u64);
    }

    // Legacy fallback (no BootInfo, e.g. bare BIOS test kernels): the bootloader
    // populated the BIB at `BIB_OFFSET`.
    // SAFETY: unchanged legacy contract — on this path the BIB is populated and the
    // low identity page is mapped by the firmware/loader.
    let bib = unsafe { &*(bios::BIB_OFFSET as *const BiosInformationBlock) };
    Ok(bib.memory_map_entries as u64)
}

/// Implements `GetBiosMemoryMapEntry()`.
///
/// Copies metadata of a specific memory map entry into user space. Prefers the
/// unified map (see [`unified_memory_map`]); only reads the low BIOS memory-map
/// array on the legacy no-`BootInfo` path.
pub fn syscall_get_bios_memory_map_entry_impl(
    index: u64,
    out_ptr: *mut UserBiosMemoryRegion,
) -> SyscallResult<u64> {
    // Step 1: resolve the requested entry, preferring the unified map. Bounds are
    // validated per source before any user write.
    let user_region = if let Some(map) = unified_memory_map() {
        let entry = map.get(index as usize).ok_or(SyscallError::InvalidArg)?;
        UserBiosMemoryRegion {
            start: entry.start,
            size: entry.size,
            region_type: efi_type_to_e820_region_type(entry.memory_type),
            _padding: 0,
        }
    } else {
        // Legacy fallback: read the low BIOS Information Block + memory-map array.
        // SAFETY: unchanged legacy contract — BIB/map populated and low page mapped.
        let bib = unsafe { &*(bios::BIB_OFFSET as *const BiosInformationBlock) };
        if index >= bib.memory_map_entries as u64 {
            return Err(SyscallError::InvalidArg);
        }
        let region = bios::MEMORYMAP_OFFSET as *const BiosMemoryRegion;
        // SAFETY: `index` validated above; `region` is a contiguous array of
        // `BiosMemoryRegion` at `MEMORYMAP_OFFSET`.
        let current_region = unsafe { &*region.add(index as usize) };
        UserBiosMemoryRegion {
            start: current_region.start,
            size: current_region.size,
            region_type: current_region.region_type,
            _padding: 0,
        }
    };

    // Step 2: Validate alignment of the user-space output pointer.
    // `UserBiosMemoryRegion` contains u64 fields and therefore requires 8-byte
    // alignment; `core::ptr::write` to a misaligned address is undefined behavior.
    if !(out_ptr as u64).is_multiple_of(core::mem::align_of::<UserBiosMemoryRegion>() as u64) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: Verify that the user-space output pointer represents a valid,
    // writable memory range in the Ring-3 address space.
    let struct_size = core::mem::size_of::<UserBiosMemoryRegion>();
    if !is_valid_user_buffer_writable(out_ptr as *const u8, struct_size) {
        return Err(SyscallError::InvalidArg);
    }

    // SAFETY:
    // - `out_ptr` has been validated to point entirely within present,
    //   user-accessible, writable pages, and is 8-byte aligned as verified above.
    // - The caller owns the memory range in user space.
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
    // Step 1: Validate alignment of the user-space output pointer.
    // `UserDateTime` contains u32 fields and requires 4-byte alignment;
    // `core::ptr::write` to a misaligned address is undefined behavior.
    if !(out_ptr as u64)
        .is_multiple_of(core::mem::align_of::<crate::syscall::types::UserDateTime>() as u64)
    {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Verify that the user-space output pointer represents a valid,
    // writable memory range in the Ring-3 address space.
    let struct_size = core::mem::size_of::<crate::syscall::types::UserDateTime>();
    if !is_valid_user_buffer_writable(out_ptr as *const u8, struct_size) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: Query the high-precision system time from the time driver.
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
    // - `out_ptr` has been validated to point entirely within present,
    //   user-accessible, writable pages.
    // - `out_ptr` is 4-byte aligned as verified above.
    // - Memory safety is preserved since the caller owns the memory range in user space.
    unsafe {
        out_ptr.write(user_dt);
    }

    Ok(0)
}
