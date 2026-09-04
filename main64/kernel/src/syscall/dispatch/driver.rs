//! Driver infrastructure syscall handlers (MMIO, IRQ bridge, SpawnDriver).

use crate::arch::constants::PAGE_SIZE_U64;
use crate::memory::vmm::{
    self, map_user_mmio_page, read_cr3, unmap_without_release, with_address_space, USER_MMIO_BASE,
    USER_STACK_GUARD_BASE,
};
use crate::process::capabilities::{Capabilities, MmioAllocKind};
use crate::scheduler;
use crate::syscall::types::{
    is_valid_user_buffer, is_valid_user_buffer_writable, SyscallError, SyscallResult, SYSCALL_OK,
};

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

    // Step 3: Compute page-aligned ranges for physical memory and virtual allocation.
    let offset_in_page = phys_addr & (PAGE_SIZE_U64 - 1);
    let page_phys_start = phys_addr & !(PAGE_SIZE_U64 - 1);
    let page_phys_end = end_phys
        .checked_add(PAGE_SIZE_U64 - 1)
        .ok_or(SyscallError::InvalidArg)?
        & !(PAGE_SIZE_U64 - 1);
    let num_bytes = page_phys_end - page_phys_start;
    let num_pages = (num_bytes / PAGE_SIZE_U64) as usize;

    // Step 4: Check fine-grained resource grants against the page-rounded
    // range that will actually be mapped, not just the caller's unrounded
    // [phys_addr, end_phys) request. A BAR smaller than or unaligned to a
    // 4KiB page (PCI legally allows BARs as small as 16 bytes) would
    // otherwise pass a grant check against the exact request while the
    // subsequent page-rounded mapping exposes whatever physical memory
    // shares that page with the granted device — a capability bypass.
    let grant_matched = caps.grants.mmio_regions.iter().any(|&(g_base, g_len)| {
        if let Some(g_end) = g_base.checked_add(g_len) {
            page_phys_start >= g_base && page_phys_end <= g_end
        } else {
            false
        }
    });
    if !grant_matched {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 5: Resolve next virtual address from the per-task MMIO bump allocator.
    if caps.grants.mmio_bump < USER_MMIO_BASE {
        caps.grants.mmio_bump = USER_MMIO_BASE;
    }
    let base_va = caps.grants.mmio_bump;

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

    // Step 7: Advance bump pointer, record the allocation's origin so a later
    // FreeDma cannot be used to release this MMIO BAR window (see
    // `DriverCaps::allocations`), and return the user virtual address.
    caps.grants.mmio_bump = end_va;
    caps.record_allocation(base_va, num_pages, MmioAllocKind::Mmio);
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

    // Step 4: Compute the page range and verify it actually originated from a
    // MapPhysical call on this task, not an AllocDma buffer. Without this
    // check, UnmapPhysical on an AllocDma VA would unmap the pages without
    // ever releasing their physical frames to the PMM, leaking them forever
    // (see `DriverCaps::allocations`).
    let page_va_start = user_va & !(PAGE_SIZE_U64 - 1);
    let page_va_end = (end_va + PAGE_SIZE_U64 - 1) & !(PAGE_SIZE_U64 - 1);
    let num_pages = ((page_va_end - page_va_start) / PAGE_SIZE_U64) as usize;

    if !caps.take_allocation(page_va_start, num_pages, MmioAllocKind::Mmio) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 5: Unmap pages across the region without releasing device physical frames to PMM.
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
/// Resource grants are derived by the **kernel** from its own PCI enumeration
/// (`drivers::driver_db`), never adopted from the caller. A caller-supplied physical
/// range would let any task holding `SPAWN_DRIVER` map arbitrary physical memory —
/// including kernel frames — into a Ring-3 address space through `MapPhysical`, and
/// thereby defeat the address-space isolation the capability system exists to provide
/// (`docs/todo_drivers.md` §3, §6). The binary name selects which device the driver may
/// bind to; `grants_ptr` is only a *request*, cross-checked against the derived grant.
///
/// Arguments:
/// - `name_ptr`: Pointer to null-terminated driver binary filename in user memory.
/// - `caps_flags`: Bitflags representing coarse capabilities (`Capabilities`), masked to
///   `driver_db::DRIVER_GRANTABLE_CAPS` so that `SPAWN_DRIVER` is never propagated.
/// - `grants_ptr`: Pointer to a `UserDriverGrants` *request* in user memory, or null to
///   accept the kernel-derived grant unconditionally.
pub fn syscall_spawn_driver_impl(
    name_ptr: *const u8,
    caps_flags: u64,
    grants_ptr: *const crate::syscall::types::UserDriverGrants,
) -> SyscallResult<u64> {
    use crate::drivers::driver_db::{self, BindError};
    use crate::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
    use crate::process::ExecError;
    use crate::syscall::types::{is_valid_user_buffer_readable, UserDriverGrants};
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    // Step 1: Verify that the caller holds SPAWN_DRIVER capability or is privileged.
    //
    // A task with no DriverCaps block (e.g. the privileged boot shell, which
    // has no capability grants of its own) falls through to the `privileged`
    // flag on its scheduler slot. Failing closed (`false`) when there is no
    // current task at all is deliberate: an authorization check must never
    // default to "allowed" just because it could not identify the caller.
    let caller_caps = scheduler::current_task_caps();
    let is_authorized = match caller_caps {
        Some(caps) => caps.flags.contains(Capabilities::SPAWN_DRIVER),
        None => scheduler::current_task_id().is_some_and(scheduler::is_task_privileged),
    };
    if !is_authorized {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 2: Read binary filename from user space.
    let name = super::fs::read_user_string(name_ptr, 128)?;

    // Step 3: Read the caller's grant *request*, if one was supplied. The values are
    // never adopted — they are only cross-checked in step 5 so that a caller asking for
    // a region outside its device is rejected loudly instead of silently downgraded.
    let requested = if !grants_ptr.is_null() {
        if !is_valid_user_buffer_readable(
            grants_ptr as *const u8,
            core::mem::size_of::<UserDriverGrants>(),
        ) {
            return Err(SyscallError::InvalidArg);
        }
        // SAFETY:
        // - `is_valid_user_buffer_readable` verified the buffer is canonical and mapped.
        // - `read_unaligned` is safe for any alignment.
        Some(unsafe { core::ptr::read_unaligned(grants_ptr) })
    } else {
        None
    };

    // Step 4: Derive the authoritative grants from the kernel's own PCI enumeration.
    // An unregistered binary receives no grants at all; it can still be spawned (it is
    // then just an ordinary Ring-3 program), but it may not carry an MMIO/IRQ request.
    //
    // A successful `derive_grants` atomically reserves `device` against a second
    // concurrent SpawnDriver for the same binary (see `driver_db::reserve_device`).
    // Every early return below this point must resolve that reservation via
    // `release_reservation`, and the success path must resolve it via
    // `confirm_binding` once the task actually exists — otherwise the device would
    // stay reserved forever with no task to release it in `remove_task`.
    let (grants, bound_device) = match driver_db::derive_grants(&name) {
        Ok((grants, device)) => {
            crate::logging::logln(
                "driver",
                format_args!(
                    "SpawnDriver: bound '{}' to PCI device {:04x}:{:04x}, IRQ {}",
                    name, device.vendor_id, device.device_id, device.interrupt_line
                ),
            );
            (grants, Some(device))
        }
        Err(BindError::UnknownDriver) => {
            // Reject a grant request for a binary the kernel does not know as a driver:
            // there is no PCI device to validate it against.
            if requested.is_some_and(|req| req.mmio_len > 0 || req.irq != 0xFF) {
                crate::logging::logln(
                    "driver",
                    format_args!(
                        "SpawnDriver: refused grant request for unregistered driver '{}'",
                        name
                    ),
                );
                return Err(SyscallError::PermissionDenied);
            }

            (
                ResourceGrants {
                    mmio_regions: Vec::new(),
                    irqs: Vec::new(),
                    mmio_bump: USER_MMIO_BASE,
                },
                None,
            )
        }
        Err(reason) => {
            crate::logging::logln(
                "driver",
                format_args!("SpawnDriver: cannot bind driver '{}': {:?}", name, reason),
            );
            return Err(SyscallError::InvalidArg);
        }
    };

    // Step 5: Reject a request that contradicts the derived grant. The caller may ask
    // for less (base 0 / IRQ 0xFF mean "no preference"), but never for something else.
    if let Some(req) = requested {
        if !driver_db::request_matches_grants(&grants, req.mmio_base, req.irq) {
            crate::logging::logln(
                "driver",
                format_args!(
                    "SpawnDriver: '{}' requested MMIO {:#x} / IRQ {} outside its device grant",
                    name, req.mmio_base, req.irq
                ),
            );
            if let Some(device) = &bound_device {
                driver_db::release_reservation(device);
            }
            return Err(SyscallError::PermissionDenied);
        }
    }

    // Step 6: Spawn the task by loading ELF executable from VFS, directly in
    // `TaskState::Blocked`. Parent linkage, the driver_db reservation, and
    // DriverCaps are not attached until steps 7-9 below; if the task were
    // schedulable immediately (as a plain `exec_from_vfs` spawn would leave
    // it), a timer tick landing in that gap could select and run — and
    // potentially crash — it before that setup ever runs, permanently
    // stranding the reservation confirmed in step 8 at `RESERVED_TASK_ID`
    // (see `driver_db::confirm_binding`). Step 10 unblocks the task once
    // setup has fully completed.
    let result = crate::process::exec_from_vfs_blocked(&name);
    let tid = match result {
        Ok(tid) => tid,
        Err(e) => {
            if let Some(device) = &bound_device {
                driver_db::release_reservation(device);
            }
            return match e {
                ExecError::InvalidName
                | ExecError::NotFound
                | ExecError::IsDirectory
                | ExecError::EmptyImage
                | ExecError::FileTooLarge
                | ExecError::InvalidElfImage => Err(SyscallError::InvalidArg),
                ExecError::OutOfMemory | ExecError::MappingFailed => Err(SyscallError::OutOfMemory),
                ExecError::SpawnFailed | ExecError::Io => Err(SyscallError::Io),
            };
        }
    };

    // Step 7: Assign parent task linkage.
    if let Some(caller_id) = scheduler::current_task_id() {
        scheduler::set_task_parent(tid, caller_id);
    }

    // Step 8: Resolve the device reservation from Step 4 to the task that now
    // actually owns it, so `remove_task` can release it again on exit.
    if let Some(device) = &bound_device {
        driver_db::confirm_binding(device, tid);
    }

    // Step 9: Construct and attach DriverCaps to newly created task. The requested
    // flags are masked to the driver-grantable set, so a driver can never inherit
    // SPAWN_DRIVER and mint further drivers with capabilities of its own choosing.
    let granted_caps = driver_db::sanitize_driver_caps(caps_flags);
    let caps = DriverCaps::new(granted_caps, grants);
    let caps_ptr = Box::into_raw(Box::new(caps));
    scheduler::set_task_caps(tid, caps_ptr);

    // Step 10: Setup is complete — allow the task to actually run. Must be
    // the last step: everything above must be visible to the task before it
    // ever executes a single instruction (see step 6).
    scheduler::unblock_task(tid);

    crate::logging::logln(
        "driver",
        format_args!(
            "SpawnDriver: created driver '{}' as task {} with caps {:#x} (requested {:#x})",
            name,
            tid,
            granted_caps.bits(),
            caps_flags
        ),
    );

    Ok(tid as u64)
}

/// Allocates physically contiguous page frames for driver DMA and maps them into the calling task.
///
/// Arguments:
/// - `pages`: Number of 4 KiB pages to allocate (must be > 0).
/// - `out_phys`: Optional user pointer to write the physical base address to (or null if not requested).
///
/// Returns the user virtual address where the DMA buffer is mapped.
pub fn syscall_alloc_dma_impl(pages: usize, out_phys: *mut u64) -> SyscallResult<u64> {
    // Step 1: Validate page count.
    if pages == 0 {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Check DriverCaps. AllocDma requires MMIO, not IRQ: the buffer is
    // mapped into the same MMIO VA window as MapPhysical/UnmapPhysical (which
    // gate on MMIO alone), and is only useful to a driver that can actually
    // program its physical address into a device register — something an
    // IRQ-only task cannot do (see `docs/drivers.md`).
    let caps = scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::MMIO) {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 3: Allocate physically contiguous frames from PMM.
    let frame = crate::memory::pmm::with_pmm(|mgr| mgr.alloc_contiguous_frames(pages))
        .ok_or(SyscallError::OutOfMemory)?;
    let base_pfn = frame.pfn;
    let base_phys = base_pfn * PAGE_SIZE_U64;

    // Step 4: Resolve next virtual address from the per-task MMIO bump allocator.
    if caps.grants.mmio_bump < USER_MMIO_BASE {
        caps.grants.mmio_bump = USER_MMIO_BASE;
    }
    let base_va = caps.grants.mmio_bump;
    let num_bytes = (pages as u64) * PAGE_SIZE_U64;
    let end_va = base_va
        .checked_add(num_bytes)
        .ok_or(SyscallError::OutOfMemory)?;
    if end_va > USER_STACK_GUARD_BASE {
        // Rollback physical allocation on VA space exhaustion.
        crate::memory::pmm::with_pmm(|mgr| {
            for i in 0..pages {
                mgr.release_pfn(base_pfn + i as u64);
            }
        });
        return Err(SyscallError::OutOfMemory);
    }

    // Step 5: Map each page in the driver's address space with uncacheable (PCD) attributes.
    let active_cr3 = read_cr3();
    let map_res = with_address_space(active_cr3, || {
        for i in 0..pages {
            let page_va = base_va + (i as u64) * PAGE_SIZE_U64;
            let page_pfn = base_pfn + (i as u64);
            map_user_mmio_page(page_va, page_pfn)?;
        }
        Ok(())
    });

    if let Err(e) = map_res {
        // Rollback mapped pages and release PMM frames.
        with_address_space(active_cr3, || {
            for i in 0..pages {
                let page_va = base_va + (i as u64) * PAGE_SIZE_U64;
                unmap_without_release(page_va);
            }
        });
        crate::memory::pmm::with_pmm(|mgr| {
            for i in 0..pages {
                mgr.release_pfn(base_pfn + i as u64);
            }
        });
        return match e {
            vmm::MapError::OutOfMemory { .. } => Err(SyscallError::OutOfMemory),
            _ => Err(SyscallError::InvalidArg),
        };
    }

    // Step 6: If out_phys pointer was provided, copy physical address to user memory.
    if !out_phys.is_null() {
        if !is_valid_user_buffer_writable(out_phys as *const u8, core::mem::size_of::<u64>()) {
            // Rollback mapping and frames if pointer is invalid.
            with_address_space(active_cr3, || {
                for i in 0..pages {
                    let page_va = base_va + (i as u64) * PAGE_SIZE_U64;
                    unmap_without_release(page_va);
                }
            });
            crate::memory::pmm::with_pmm(|mgr| {
                for i in 0..pages {
                    mgr.release_pfn(base_pfn + i as u64);
                }
            });
            return Err(SyscallError::InvalidArg);
        }
        // SAFETY:
        // - `is_valid_user_buffer_writable` verified the pointer range is
        //   canonical user space and mapped present+writable in the page
        //   table, so this write cannot fault.
        // - `write_unaligned` safely writes 8 bytes.
        unsafe {
            core::ptr::write_unaligned(out_phys, base_phys);
        }
    }

    // Step 7: Advance bump pointer, record the allocation's origin so a later
    // UnmapPhysical cannot be used to leak this buffer's RAM frames (see
    // `DriverCaps::allocations`), and return the virtual address.
    caps.grants.mmio_bump = end_va;
    caps.record_allocation(base_va, pages, MmioAllocKind::Dma);
    Ok(base_va)
}

/// Frees previously allocated driver DMA pages and unmaps them from the task.
///
/// Arguments:
/// - `user_va`: User virtual starting address returned by `AllocDma`.
/// - `pages`: Number of 4 KiB pages.
pub fn syscall_free_dma_impl(user_va: u64, pages: usize) -> SyscallResult<u64> {
    // Step 1: Validate parameters.
    if pages == 0 {
        return Err(SyscallError::InvalidArg);
    }
    let num_bytes = (pages as u64)
        .checked_mul(PAGE_SIZE_U64)
        .ok_or(SyscallError::InvalidArg)?;
    let end_va = user_va
        .checked_add(num_bytes)
        .ok_or(SyscallError::InvalidArg)?;

    // Step 2: Check DriverCaps. Mirrors AllocDma's MMIO-only gate (see its
    // Step 2 comment) — a task that could allocate a DMA buffer without MMIO
    // could not free one without it either.
    let caps = scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::MMIO) {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 3: Validate virtual address range.
    if user_va < USER_MMIO_BASE || end_va > USER_STACK_GUARD_BASE {
        return Err(SyscallError::InvalidArg);
    }

    // Step 4: Reserve bookkeeping storage before mutating any state. Doing this
    // before `take_allocation` ensures an OOM here fails cleanly with the
    // allocation record still intact, instead of removing the record and then
    // bailing out with the VA still mapped and the frames still unreleased.
    let mut pfns_to_release = alloc::vec::Vec::new();
    if pfns_to_release.try_reserve(pages).is_err() {
        return Err(SyscallError::OutOfMemory);
    }

    // Step 5: Verify this VA range actually originated from an AllocDma call
    // on this task, not a MapPhysical BAR window. Without this check, FreeDma
    // on a MapPhysical VA would unmap a device's register window and then
    // call `pmm::release_pfn` on its physical BAR address — silent corruption
    // instead of a rejected call (see `DriverCaps::allocations`).
    if !caps.take_allocation(user_va, pages, MmioAllocKind::Dma) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 6: Resolve PFNs, unmap virtual pages, and release physical frames to PMM.
    let active_cr3 = read_cr3();

    with_address_space(active_cr3, || {
        for i in 0..pages {
            let page_va = user_va + (i as u64) * PAGE_SIZE_U64;
            if let Some(phys) = vmm::virt_to_phys_current(page_va) {
                pfns_to_release.push(phys / PAGE_SIZE_U64);
            }
            unmap_without_release(page_va);
        }
    });

    crate::memory::pmm::with_pmm(|mgr| {
        for pfn in pfns_to_release {
            mgr.release_pfn(pfn);
        }
    });

    Ok(SYSCALL_OK)
}

/// Translates a virtual address in the calling task's address space to its physical address.
///
/// Arguments:
/// - `user_va`: Virtual address to translate.
pub fn syscall_virt_to_phys_impl(user_va: u64) -> SyscallResult<u64> {
    // Step 1: Verify task capabilities.
    let caps = scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::MMIO) && !caps.flags.contains(Capabilities::IRQ) {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 2: Reject non-canonical or kernel-half addresses before walking the
    // page tables, so a driver task cannot probe kernel virtual memory (which
    // is mapped into every user PML4) to leak its physical layout.
    if !is_valid_user_buffer(user_va as *const u8, 1) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: Translate virtual address via active page-table walk.
    vmm::virt_to_phys_current(user_va).ok_or(SyscallError::InvalidArg)
}
