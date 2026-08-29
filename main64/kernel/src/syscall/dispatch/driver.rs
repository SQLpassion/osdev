//! Driver infrastructure syscall handlers (MMIO, IRQ bridge, SpawnDriver).

use crate::arch::constants::PAGE_SIZE_U64;
use crate::memory::vmm::{
    self, map_user_mmio_page, read_cr3, unmap_without_release, with_address_space, USER_MMIO_BASE,
    USER_STACK_GUARD_BASE,
};
use crate::process::capabilities::Capabilities;
use crate::scheduler;
use crate::syscall::types::{SyscallError, SyscallResult, SYSCALL_OK};

/// Maps a physical MMIO region into the calling driver task's address space.
///
/// Arguments:
/// - `phys_addr`: Physical starting address of the device register window.
/// - `len`: Size of the region in bytes.
/// - `_flags`: Reserved for future mapping attributes (must be 0).
pub fn syscall_map_physical_impl(phys_addr: u64, len: usize, _flags: u64) -> SyscallResult<u64> {
    // Step 1: Validate length and ensure address arithmetic does not overflow.
    if len == 0 {
        return Err(SyscallError::InvalidArg);
    }
    let end_phys = phys_addr
        .checked_add(len as u64)
        .ok_or(SyscallError::InvalidArg)?;

    // Step 2: Retrieve DriverCaps and verify coarse MMIO capability.
    let caps = scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::MMIO) {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 3: Check fine-grained resource grants (phys_addr..end_phys must be covered by a grant).
    let grant_matched = caps.grants.mmio_regions.iter().any(|&(g_base, g_len)| {
        if let Some(g_end) = g_base.checked_add(g_len) {
            phys_addr >= g_base && end_phys <= g_end
        } else {
            false
        }
    });
    if !grant_matched {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 4: Resolve next virtual address from the per-task MMIO bump allocator.
    if caps.grants.mmio_bump < USER_MMIO_BASE {
        caps.grants.mmio_bump = USER_MMIO_BASE;
    }
    let base_va = caps.grants.mmio_bump;

    // Step 5: Compute page-aligned ranges for physical memory and virtual allocation.
    let offset_in_page = phys_addr & (PAGE_SIZE_U64 - 1);
    let page_phys_start = phys_addr & !(PAGE_SIZE_U64 - 1);
    let page_phys_end = (end_phys + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1);
    let num_bytes = page_phys_end - page_phys_start;
    let num_pages = (num_bytes / PAGE_SIZE_U64) as usize;

    let end_va = base_va
        .checked_add(num_bytes)
        .ok_or(SyscallError::OutOfMemory)?;
    if end_va > USER_STACK_GUARD_BASE {
        return Err(SyscallError::OutOfMemory);
    }

    // Step 6: Map each page in the driver's active address space with uncacheable (PCD) attributes.
    let active_cr3 = read_cr3();
    let map_res = with_address_space(active_cr3, || {
        for i in 0..num_pages {
            let page_va = base_va + (i as u64) * PAGE_SIZE_U64;
            let page_pfn = (page_phys_start + (i as u64) * PAGE_SIZE_U64) / PAGE_SIZE_U64;
            map_user_mmio_page(page_va, page_pfn)?;
        }
        Ok(())
    });

    if let Err(e) = map_res {
        // Rollback any successfully mapped pages on failure.
        with_address_space(active_cr3, || {
            for i in 0..num_pages {
                let page_va = base_va + (i as u64) * PAGE_SIZE_U64;
                unmap_without_release(page_va);
            }
        });
        return match e {
            vmm::MapError::OutOfMemory { .. } => Err(SyscallError::OutOfMemory),
            _ => Err(SyscallError::InvalidArg),
        };
    }

    // Step 7: Advance bump pointer and return the user virtual address.
    caps.grants.mmio_bump = end_va;
    Ok(base_va + offset_in_page)
}

/// Unmaps a previously mapped MMIO region from the calling driver task's address space.
///
/// Arguments:
/// - `user_va`: User virtual starting address returned by `MapPhysical`.
/// - `len`: Size of the region in bytes.
pub fn syscall_unmap_physical_impl(user_va: u64, len: usize) -> SyscallResult<u64> {
    // Step 1: Validate length and ensure address arithmetic does not overflow.
    if len == 0 {
        return Err(SyscallError::InvalidArg);
    }
    let end_va = user_va
        .checked_add(len as u64)
        .ok_or(SyscallError::InvalidArg)?;

    // Step 2: Retrieve DriverCaps and verify coarse MMIO capability.
    let caps = scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::MMIO) {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 3: Validate that virtual address range lies within the user MMIO window.
    if user_va < USER_MMIO_BASE || end_va > USER_STACK_GUARD_BASE {
        return Err(SyscallError::InvalidArg);
    }

    // Step 4: Unmap pages across the region without releasing device physical frames to PMM.
    let page_va_start = user_va & !(PAGE_SIZE_U64 - 1);
    let page_va_end = (end_va + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1);
    let num_pages = ((page_va_end - page_va_start) / PAGE_SIZE_U64) as usize;

    let active_cr3 = read_cr3();
    with_address_space(active_cr3, || {
        for i in 0..num_pages {
            let page_va = page_va_start + (i as u64) * PAGE_SIZE_U64;
            unmap_without_release(page_va);
        }
    });

    Ok(SYSCALL_OK)
}

/// Helper that verifies the calling task holds `Capabilities::IRQ` and has been
/// granted access to `vector` in its `ResourceGrants`.
fn verify_irq_permission(vector: u64) -> SyscallResult<usize> {
    let vector_u8 = u8::try_from(vector).map_err(|_| SyscallError::InvalidArg)?;
    let irq_idx =
        crate::drivers::irq_bridge::irq_to_index(vector_u8).ok_or(SyscallError::InvalidArg)? as u8;
    let irq_vec = crate::drivers::irq_bridge::irq_index_to_vector(irq_idx as usize);

    let caps = scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::IRQ) {
        return Err(SyscallError::PermissionDenied);
    }

    let is_granted = caps.grants.irqs.contains(&vector_u8)
        || caps.grants.irqs.contains(&irq_idx)
        || caps.grants.irqs.contains(&irq_vec);
    if !is_granted {
        return Err(SyscallError::PermissionDenied);
    }

    let current_id = scheduler::current_task_id().ok_or(SyscallError::PermissionDenied)?;
    Ok(current_id)
}

/// Subscribes the current task to an IRQ vector.
///
/// Arguments:
/// - `vector`: Hardware IRQ line (0..15) or IDT vector (32..47).
pub fn syscall_irq_subscribe_impl(vector: u64) -> SyscallResult<u64> {
    let current_id = verify_irq_permission(vector)?;
    let vector_u8 = vector as u8;

    crate::drivers::irq_bridge::subscribe(vector_u8, current_id)?;
    Ok(SYSCALL_OK)
}

/// Blocks the current task until the subscribed IRQ fires (or timeout).
///
/// Arguments:
/// - `vector`: Hardware IRQ line (0..15) or IDT vector (32..47).
/// - `timeout_ms`: Maximum wait duration in milliseconds (0 = infinite).
pub fn syscall_irq_wait_impl(vector: u64, timeout_ms: u64) -> SyscallResult<u64> {
    let current_id = verify_irq_permission(vector)?;
    let vector_u8 = vector as u8;
    let timeout_u32 = u32::try_from(timeout_ms).unwrap_or(u32::MAX);

    crate::drivers::irq_bridge::wait(vector_u8, current_id, timeout_u32)?;
    Ok(SYSCALL_OK)
}

/// Acknowledges an IRQ event, triggering PIC EOI.
///
/// Arguments:
/// - `vector`: Hardware IRQ line (0..15) or IDT vector (32..47).
pub fn syscall_irq_ack_impl(vector: u64) -> SyscallResult<u64> {
    let current_id = verify_irq_permission(vector)?;
    let vector_u8 = vector as u8;

    crate::drivers::irq_bridge::ack(vector_u8, current_id)?;
    Ok(SYSCALL_OK)
}

/// Spawns a user-space driver process with dedicated capabilities and resource grants.
///
/// Arguments:
/// - `name_ptr`: Pointer to null-terminated driver binary filename in user memory.
/// - `caps_flags`: Bitflags representing coarse capabilities (`Capabilities`).
/// - `grants_ptr`: Pointer to `UserDriverGrants` struct in user memory (or null for empty grants).
pub fn syscall_spawn_driver_impl(
    name_ptr: *const u8,
    caps_flags: u64,
    grants_ptr: *const crate::syscall::types::UserDriverGrants,
) -> SyscallResult<u64> {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use crate::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
    use crate::process::ExecError;
    use crate::syscall::types::{is_valid_user_buffer_readable, UserDriverGrants};

    // Step 1: Verify that the caller holds SPAWN_DRIVER capability or is privileged.
    let caller_caps = scheduler::current_task_caps();
    let is_authorized = match caller_caps {
        Some(caps) => caps.flags.contains(Capabilities::SPAWN_DRIVER),
        None => {
            if let Some(tid) = scheduler::current_task_id() {
                scheduler::is_task_privileged(tid) || tid == 1
            } else {
                true
            }
        }
    };
    if !is_authorized {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 2: Read binary filename from user space.
    let name = super::fs::read_user_string(name_ptr, 128)?;

    // Step 3: Parse UserDriverGrants if supplied.
    let grants = if !grants_ptr.is_null() {
        if !is_valid_user_buffer_readable(
            grants_ptr as *const u8,
            core::mem::size_of::<UserDriverGrants>(),
        ) {
            return Err(SyscallError::InvalidArg);
        }
        // SAFETY:
        // - `is_valid_user_buffer_readable` verified the buffer is canonical and mapped.
        // - `read_unaligned` is safe for any alignment.
        let raw_grants = unsafe { core::ptr::read_unaligned(grants_ptr) };

        let mut mmio_regions = Vec::new();
        if raw_grants.mmio_len > 0 {
            mmio_regions.push((raw_grants.mmio_base, raw_grants.mmio_len));
        }

        let mut irqs = Vec::new();
        if raw_grants.irq != 0xFF {
            irqs.push(raw_grants.irq);
        }

        ResourceGrants {
            mmio_regions,
            irqs,
            mmio_bump: USER_MMIO_BASE,
        }
    } else {
        ResourceGrants {
            mmio_regions: Vec::new(),
            irqs: Vec::new(),
            mmio_bump: USER_MMIO_BASE,
        }
    };

    // Step 4: Spawn the task by loading ELF executable from VFS.
    let result = crate::process::exec_from_vfs(&name);
    let tid = match result {
        Ok(tid) => tid,
        Err(ExecError::FileNotFound) => return Err(SyscallError::Io),
        Err(ExecError::CorruptImage) | Err(ExecError::InvalidElf) => {
            return Err(SyscallError::InvalidArg)
        }
        Err(ExecError::ImageTooLarge) | Err(ExecError::OutOfMemory) => {
            return Err(SyscallError::OutOfMemory)
        }
        Err(ExecError::SpawnFailed) => return Err(SyscallError::Io),
    };

    // Step 5: Assign parent task linkage.
    if let Some(caller_id) = scheduler::current_task_id() {
        scheduler::set_task_parent(tid, caller_id);
    }

    // Step 6: Construct and attach DriverCaps to newly created task.
    let caps = DriverCaps::new(Capabilities::from_bits_truncate(caps_flags), grants);
    let caps_ptr = Box::into_raw(Box::new(caps));
    scheduler::set_task_caps(tid, caps_ptr);

    crate::logging::logln(
        "driver",
        format_args!(
            "SpawnDriver: created driver '{}' as task {} with caps {:#x}",
            name, tid, caps_flags
        ),
    );

    Ok(tid as u64)
}

