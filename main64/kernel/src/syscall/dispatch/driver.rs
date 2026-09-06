//! Driver infrastructure syscall handlers (MMIO, DMA, SpawnDriver).

use crate::arch::constants::PAGE_SIZE_U64;
use crate::drivers::{registry, time};
use crate::memory::vmm::{
    self, map_user_mmio_page, read_cr3, unmap_without_release, with_address_space, USER_MMIO_BASE,
    USER_STACK_GUARD_BASE,
};
use crate::process::capabilities::{Capabilities, MmioAllocKind};
use crate::scheduler;
use crate::syscall::types::{
    is_valid_user_buffer, is_valid_user_buffer_readable, is_valid_user_buffer_writable,
    SyscallError, SyscallResult, UserDriverInfo, UserDriverStatus, MAX_ARP_ENTRIES, SYSCALL_OK,
};

/// Maps a physical MMIO region into the calling driver task's address space.
///
/// Arguments:
/// - `phys_addr`: Physical starting address of the device register window.
/// - `len`: Size of the region in bytes.
pub fn syscall_map_physical_impl(phys_addr: u64, len: usize) -> SyscallResult<u64> {
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
    // then just an ordinary Ring-3 program), but it may not carry an MMIO request.
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
                    "SpawnDriver: bound '{}' to PCI device {:04x}:{:04x}",
                    name, device.vendor_id, device.device_id
                ),
            );
            (grants, Some(device))
        }
        Err(BindError::UnknownDriver) => {
            // Reject a grant request for a binary the kernel does not know as a driver:
            // there is no PCI device to validate it against.
            if requested.is_some_and(|req| req.mmio_len > 0) {
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
    // for less (base 0 means "no preference"), but never for something else.
    if let Some(req) = requested {
        if !driver_db::request_matches_grants(&grants, req.mmio_base) {
            crate::logging::logln(
                "driver",
                format_args!(
                    "SpawnDriver: '{}' requested MMIO {:#x} outside its device grant",
                    name, req.mmio_base
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

    // Step 7: Assign parent task linkage. The task spawned in step 6 is
    // still `Blocked` and therefore cannot have exited yet, so
    // `set_task_parent` returning `false` here means the slot was somehow
    // already invalidated out from under us — an internal inconsistency
    // serious enough to abort the spawn rather than silently return
    // `Ok(tid)` for a task with no recorded parent (which would make a later
    // `Wait` on `tid` behave as "not my child" instead of surfacing the real
    // problem).
    if let Some(caller_id) = scheduler::current_task_id() {
        if !scheduler::set_task_parent(tid, caller_id) {
            crate::logging::logln(
                "driver",
                format_args!(
                    "SpawnDriver: set_task_parent({}, {}) failed — aborting spawn",
                    tid, caller_id
                ),
            );
            scheduler::terminate_task(tid);
            return Err(SyscallError::Io);
        }
    }

    // Step 8: Resolve the device reservation from Step 4 to the task that now
    // actually owns it, so `remove_task` can release it again on exit. Must
    // succeed for the same reason as step 7 — the task cannot have exited
    // yet, so a `false` return means the reservation this task is supposed
    // to own was somehow already lost. Proceeding anyway would return
    // `Ok(tid)` for a driver task `remove_task` can never find a binding to
    // release for, permanently stranding it at `RESERVED_TASK_ID`.
    if let Some(device) = &bound_device {
        if !driver_db::confirm_binding(device, tid) {
            crate::logging::logln(
                "driver",
                format_args!(
                    "SpawnDriver: confirm_binding failed for task {} — aborting spawn",
                    tid
                ),
            );
            scheduler::terminate_task(tid);
            return Err(SyscallError::Io);
        }
    }

    // Step 9: Construct and attach DriverCaps to newly created task. The requested
    // flags are masked to the driver-grantable set, so a driver can never inherit
    // SPAWN_DRIVER and mint further drivers with capabilities of its own choosing.
    //
    // Allocated manually (instead of `Box::new`) so an OOM here returns
    // `SyscallError::OutOfMemory` like every other fallible step in this
    // function, instead of invoking the global alloc-error handler (abort),
    // which would turn a per-task resource exhaustion into a full kernel
    // panic. The task is still `Blocked` (see step 6) and has therefore never
    // run, so it can be torn down wholesale via `terminate_task` — which also
    // releases the `driver_db` reservation `confirm_binding` just assigned to
    // `tid` in step 8.
    let granted_caps = driver_db::sanitize_driver_caps(caps_flags);
    let caps = DriverCaps::new(granted_caps, grants);
    let caps_layout = core::alloc::Layout::new::<DriverCaps>();
    // SAFETY: `caps_layout` is a valid, non-zero-sized layout for `DriverCaps`.
    let caps_ptr = unsafe { alloc::alloc::alloc(caps_layout) } as *mut DriverCaps;
    if caps_ptr.is_null() {
        scheduler::terminate_task(tid);
        return Err(SyscallError::OutOfMemory);
    }
    // SAFETY:
    // - `caps_ptr` was just allocated with the layout of `DriverCaps` and
    //   holds no initialized value yet.
    // - `write` moves `caps` into it without dropping any prior contents,
    //   matching `Box::from_raw`'s expectations for the later `remove_task`
    //   deallocation (same global allocator, same layout).
    unsafe { caps_ptr.write(caps) };
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

    // Step 2: Check DriverCaps. AllocDma requires MMIO: the buffer is mapped
    // into the same MMIO VA window as MapPhysical/UnmapPhysical (which gate
    // on MMIO alone), and is only useful to a driver that can actually
    // program its physical address into a device register.
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
    if !caps.flags.contains(Capabilities::MMIO) {
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

/// Copies a user-space driver name into a kernel-owned, fixed-size buffer.
///
/// Shared by `syscall_drv_register_impl` and `syscall_drv_lookup_impl`: both
/// take a `(name_ptr, name_len)` pair rather than a NUL-terminated string, so
/// `fs::read_user_string`'s NUL-scanning does not apply here.
///
/// Returns `InvalidArg` if `name_len` is zero or exceeds
/// `registry::DRIVER_NAME_LEN`, or if `[name_ptr, name_ptr + name_len)` is not
/// a valid, mapped, user-readable buffer.
fn copy_user_driver_name(
    name_ptr: u64,
    name_len: u64,
) -> SyscallResult<([u8; registry::DRIVER_NAME_LEN], usize)> {
    // Step 1: bound-check the length before touching the pointer at all.
    if name_len == 0 || name_len > registry::DRIVER_NAME_LEN as u64 {
        return Err(SyscallError::InvalidArg);
    }
    let name_len = name_len as usize;
    let name_ptr = name_ptr as *const u8;

    // Step 2: validate the buffer is canonical, in range, and actually mapped
    // present+readable in the caller's address space before dereferencing it
    // — mirrors syscall_spawn_driver_impl's use of the same check before its
    // own `read_unaligned` of a user pointer.
    if !is_valid_user_buffer_readable(name_ptr, name_len) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: copy the name into a kernel-owned buffer. Never trust the user
    // pointer past this point — later logic only touches `name_buf`.
    let mut name_buf = [0u8; registry::DRIVER_NAME_LEN];
    // SAFETY:
    // - `is_valid_user_buffer_readable` verified `[name_ptr, name_ptr +
    //   name_len)` is canonical, in-range, and mapped present+readable in the
    //   currently active address space.
    // - `name_len <= registry::DRIVER_NAME_LEN`, so the destination buffer is
    //   large enough for the copy.
    // - The two ranges cannot overlap: `name_buf` is a fresh stack allocation.
    unsafe {
        core::ptr::copy_nonoverlapping(name_ptr, name_buf.as_mut_ptr(), name_len);
    }

    Ok((name_buf, name_len))
}

/// Registers the calling driver task under `name` (e.g. "nic:rtl8139"), so
/// applications can resolve it later via `DrvLookup`.
///
/// Arguments:
/// - `name_ptr`: Pointer to the driver name bytes in user memory (not
///   required to be NUL-terminated).
/// - `name_len`: Length of the name in bytes; must be
///   `0 < name_len <= registry::DRIVER_NAME_LEN`.
///
/// Requires the caller to be a driver task (holds a non-null `DriverCaps`
/// block — i.e. it was spawned via `SpawnDriver`). Ordinary Ring-3 apps have
/// no `DriverCaps` and must not be able to squat a driver name.
pub fn syscall_drv_register_impl(name_ptr: u64, name_len: u64) -> SyscallResult<u64> {
    // Step 1: caller must be a driver task.
    scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;

    // Step 2: validate and copy the name out of user memory.
    let (name_buf, name_len) = copy_user_driver_name(name_ptr, name_len)?;

    // Step 3: register under this task's packed id.
    let tid = scheduler::current_task_id().ok_or(SyscallError::PermissionDenied)?;
    registry::register(&name_buf[..name_len], tid)?;
    Ok(SYSCALL_OK)
}

/// Resolves the packed task id of a driver previously registered via
/// `DrvRegister`.
///
/// Arguments:
/// - `name_ptr`: Pointer to the driver name bytes in user memory (not
///   required to be NUL-terminated).
/// - `name_len`: Length of the name in bytes; must be
///   `0 < name_len <= registry::DRIVER_NAME_LEN`.
///
/// Callable by any task — resolving a driver's name is not itself a
/// privileged operation; only registering one is.
pub fn syscall_drv_lookup_impl(name_ptr: u64, name_len: u64) -> SyscallResult<u64> {
    // Step 1: validate and copy the name out of user memory.
    let (name_buf, name_len) = copy_user_driver_name(name_ptr, name_len)?;

    // Step 2: resolve the registered driver's packed task id.
    registry::lookup(&name_buf[..name_len])
        .map(|tid| tid as u64)
        .ok_or(SyscallError::InvalidArg)
}

/// Sends a raw packet to/from a driver channel.
///
/// **Role-based direction** (see the design-decision note on Phase 2 Step 2
/// / this syscall's GitHub issue for the full rationale — this resolves an
/// inconsistency in `docs/nic_driver_design.md` §4.4 vs §4.5, which describes
/// six conceptual operations but allocates only two syscall numbers here):
/// - Caller is an ordinary app (its own tid != `driver_id`): pushes into
///   `driver_id`'s TX ring (App → Driver).
/// - Caller **is** the driver itself (its own tid == `driver_id`): pushes
///   into its own RX ring (Driver → App).
///
/// Arguments:
/// - `driver_id`: Packed task id of the target driver channel (from `DrvLookup`,
///   or the caller's own tid if it is that driver).
/// - `packet_ptr`/`packet_len`: Raw packet bytes in user memory;
///   `0 < packet_len <= registry::MAX_PACKET_LEN`.
pub fn syscall_net_send_impl(
    driver_id: u64,
    packet_ptr: u64,
    packet_len: u64,
) -> SyscallResult<u64> {
    // Step 1: validate the packet length before touching the pointer.
    if packet_len == 0 || packet_len > registry::MAX_PACKET_LEN as u64 {
        return Err(SyscallError::InvalidArg);
    }
    let packet_len = packet_len as usize;
    let packet_ptr = packet_ptr as *const u8;

    // Step 2: validate the buffer is canonical, in range, and mapped
    // present+readable before dereferencing it.
    if !is_valid_user_buffer_readable(packet_ptr, packet_len) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: copy the packet into a kernel-local buffer. Never trust the
    // user pointer past this point.
    let mut packet_buf = [0u8; registry::MAX_PACKET_LEN];
    // SAFETY:
    // - `is_valid_user_buffer_readable` verified `[packet_ptr, packet_ptr +
    //   packet_len)` is canonical, in-range, and mapped present+readable.
    // - `packet_len <= registry::MAX_PACKET_LEN`, so the destination buffer
    //   is large enough.
    // - The two ranges cannot overlap: `packet_buf` is a fresh stack allocation.
    unsafe {
        core::ptr::copy_nonoverlapping(packet_ptr, packet_buf.as_mut_ptr(), packet_len);
    }

    // Step 4: role-based direction — see the doc comment above.
    let caller_is_driver = scheduler::current_task_id() == Some(driver_id as usize);

    // Step 5+6: push into the selected ring. Never blocks: a full ring is
    // backpressure to the sender (registry::PacketRing::push never blocks).
    registry::push_packet(
        driver_id as usize,
        caller_is_driver,
        &packet_buf[..packet_len],
    )?;
    Ok(SYSCALL_OK)
}

/// Receives a raw packet to/from a driver channel, mirroring `NetSend`'s
/// role-based direction (see its doc comment for the full rationale).
///
/// Arguments:
/// - `driver_id`: Packed task id of the target driver channel.
/// - `buf_ptr`/`buf_len`: Destination buffer in user memory. A packet larger
///   than `buf_len` is truncated to `buf_len` bytes, matching the truncating
///   `NicDevice::poll_next_packet` convention already used by the drivers.
/// - `timeout_ms`: `0` polls once and returns `Timeout` immediately if the
///   ring is empty; a non-zero value blocks up to that many milliseconds.
///   This is a deliberate choice: the background driver event loop
///   (`docs/nic_driver_design.md` §4.6) drains its TX ring every iteration
///   and must never block doing so.
///
/// Returns the number of bytes copied into `buf_ptr`, or `SysError::Timeout`
/// if the ring is still empty once the wait (if any) elapses.
pub fn syscall_net_recv_impl(
    driver_id: u64,
    buf_ptr: u64,
    buf_len: u64,
    timeout_ms: u64,
) -> SyscallResult<u64> {
    // Step 1: validate the destination buffer up front — every return path
    // below eventually writes into it.
    let buf_len = buf_len as usize;
    let buf_ptr = buf_ptr as *mut u8;
    if !is_valid_user_buffer_writable(buf_ptr as *const u8, buf_len) {
        return Err(SyscallError::InvalidArg);
    }

    let caller_is_driver = scheduler::current_task_id() == Some(driver_id as usize);
    let copy_cap = buf_len.min(registry::MAX_PACKET_LEN);
    let mut kernel_buf = [0u8; registry::MAX_PACKET_LEN];

    // Step 2: fast path — try once regardless of timeout_ms, so a packet
    // that is already queued is never delayed by the polling loop below.
    if let Some(n) = registry::try_pop_packet(
        driver_id as usize,
        caller_is_driver,
        &mut kernel_buf[..copy_cap],
    )? {
        // SAFETY:
        // - `is_valid_user_buffer_writable` verified `buf_ptr` is canonical,
        //   in-range, and mapped present+writable for `buf_len` bytes.
        // - `n <= copy_cap <= buf_len`, so the write stays in bounds.
        unsafe {
            core::ptr::copy_nonoverlapping(kernel_buf.as_ptr(), buf_ptr, n);
        }
        return Ok(n as u64);
    }

    // Step 3: timeout_ms == 0 means "poll once, non-blocking" here — see the
    // doc comment above.
    if timeout_ms == 0 {
        return Err(SyscallError::Timeout);
    }

    // Step 4: bounded wait. This cannot use `scheduler::block_task`: once
    // blocked, this task would only ever resume when something else
    // unblocks it, which defeats the timeout entirely if no producer ever
    // shows up. Poll cooperatively via `yield_now()` against a TSC deadline
    // instead.
    let ticks_per_ms = time::tsc_ticks_per_us().saturating_mul(1000);
    let deadline = time::rdtsc().saturating_add(ticks_per_ms.saturating_mul(timeout_ms));

    loop {
        scheduler::yield_now();

        match registry::try_pop_packet(
            driver_id as usize,
            caller_is_driver,
            &mut kernel_buf[..copy_cap],
        ) {
            Ok(Some(n)) => {
                // SAFETY: identical justification as the fast path above.
                unsafe {
                    core::ptr::copy_nonoverlapping(kernel_buf.as_ptr(), buf_ptr, n);
                }
                return Ok(n as u64);
            }
            Ok(None) => {}           // still empty — keep waiting
            Err(e) => return Err(e), // the driver exited mid-wait
        }

        if time::rdtsc() >= deadline {
            return Err(SyscallError::Timeout);
        }
    }
}

/// Publishes the calling driver task's current status snapshot for later
/// `DrvQuery` reads.
///
/// Arguments:
/// - `status_ptr`: Pointer to a `UserDriverStatus` in user memory.
///
/// Requires the caller to already be registered via `DrvRegister` (i.e. a
/// `DriverEntry` for its own tid must exist).
pub fn syscall_drv_publish_status_impl(status_ptr: u64) -> SyscallResult<u64> {
    let status_ptr = status_ptr as *const UserDriverStatus;

    // Step 1: validate the buffer is canonical, in range, and mapped
    // present+readable before dereferencing it.
    if !is_valid_user_buffer_readable(
        status_ptr as *const u8,
        core::mem::size_of::<UserDriverStatus>(),
    ) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: copy the whole struct in at once -- never trust a single field
    // read across the boundary, matching how syscall_spawn_driver_impl
    // copies UserDriverGrants in.
    // SAFETY:
    // - `is_valid_user_buffer_readable` verified the pointed-to range is
    //   canonical, in-range, and mapped present+readable for
    //   `size_of::<UserDriverStatus>()` bytes.
    // - `read_unaligned` is safe for any alignment.
    let status = unsafe { core::ptr::read_unaligned(status_ptr) };

    // Step 3: reject an arp_entry_count a misbehaving/buggy driver set past
    // the fixed-size array's actual capacity, rather than trusting it for
    // later out-of-bounds reads in DrvQuery / its own consumers.
    if status.arp_entry_count as usize > MAX_ARP_ENTRIES {
        return Err(SyscallError::InvalidArg);
    }

    // Step 4: store under this task's packed id.
    let tid = scheduler::current_task_id().ok_or(SyscallError::PermissionDenied)?;
    registry::publish_status(tid, status)?;
    Ok(SYSCALL_OK)
}

/// Reads the last status snapshot published by a driver.
///
/// Arguments:
/// - `driver_id`: Packed task id of the driver (from `DrvLookup`).
/// - `out_ptr`: Destination `UserDriverStatus` in user memory.
///
/// Returns `InvalidArg` if `driver_id` is unknown or has never published.
/// Callable by any task.
pub fn syscall_drv_query_impl(driver_id: u64, out_ptr: u64) -> SyscallResult<u64> {
    // Step 1: resolve the snapshot before validating the output buffer, so
    // an unknown/never-published driver_id fails cheaply without touching
    // user memory at all.
    let status = registry::query_status(driver_id as usize).ok_or(SyscallError::InvalidArg)?;

    // Step 2: validate the destination buffer.
    let out_ptr = out_ptr as *mut UserDriverStatus;
    if !is_valid_user_buffer_writable(
        out_ptr as *const u8,
        core::mem::size_of::<UserDriverStatus>(),
    ) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: copy the snapshot out.
    // SAFETY:
    // - `is_valid_user_buffer_writable` verified the destination range is
    //   canonical, in-range, and mapped present+writable for
    //   `size_of::<UserDriverStatus>()` bytes.
    // - `write_unaligned` is safe for any alignment.
    unsafe {
        core::ptr::write_unaligned(out_ptr, status);
    }
    Ok(SYSCALL_OK)
}

/// Terminates a registered driver task by name.
///
/// Arguments:
/// - `name_ptr`/`name_len`: driver name bytes in user memory (not required
///   to be NUL-terminated); same contract as `DrvRegister`/`DrvLookup`.
///
/// # Authorization
/// Requires the caller to hold `Capabilities::UNLOAD_DRIVER` or the
/// privileged-syscall flag — mirrors `syscall_spawn_driver_impl`'s own gate
/// (Step 1 there). An ordinary Ring-3 app must not be able to kill an
/// arbitrary driver task; only `DRIVERS.BIN`, delegated this capability by
/// the shell at `Exec` time (see `resolve_delegated_capabilities`), can.
///
/// # This is a hard kill
/// Calls [`scheduler::terminate_task`], which does **not** run the driver's
/// own shutdown path (no `drop(device)`, no disabling DMA/bus-mastering) —
/// see `docs/drivers.md` §15. Without an IOMMU, a still-DMA-active NIC could
/// in principle write into freed memory after this call returns. This is an
/// accepted risk of this kernel's current driver model, not addressed here.
pub fn syscall_drv_unload_impl(name_ptr: u64, name_len: u64) -> SyscallResult<u64> {
    // Step 1: verify that the caller holds UNLOAD_DRIVER or is privileged.
    // Same shape as `syscall_spawn_driver_impl`'s Step 1: a task with no
    // `DriverCaps` block falls through to the `privileged` scheduler flag,
    // and a missing current task fails closed rather than defaulting to
    // "allowed".
    let caller_caps = scheduler::current_task_caps();
    let is_authorized = match caller_caps {
        Some(caps) => caps.flags.contains(Capabilities::UNLOAD_DRIVER),
        None => scheduler::current_task_id().is_some_and(scheduler::is_task_privileged),
    };
    if !is_authorized {
        return Err(SyscallError::PermissionDenied);
    }

    // Step 2: validate and copy the name out of user memory.
    let (name_buf, name_len) = copy_user_driver_name(name_ptr, name_len)?;

    // Step 3: resolve the registered driver's packed task id.
    let tid = registry::lookup(&name_buf[..name_len]).ok_or(SyscallError::InvalidArg)?;

    // Step 4: hard-kill the task. `terminate_task` -> `remove_task` already
    // releases this driver's registry entry, MMIO/DMA allocations, and PCI
    // device reservation (see `manager.rs::remove_task`).
    if !scheduler::terminate_task(tid) {
        return Err(SyscallError::InvalidArg);
    }
    Ok(SYSCALL_OK)
}

/// Copies metadata for every currently registered driver into a user-space
/// buffer, up to `max_entries` entries, in one syscall.
///
/// Arguments:
/// - `out_ptr`: User pointer to an array of at least `max_entries` writable
///   [`UserDriverInfo`] slots. Never dereferenced if `max_entries == 0`.
/// - `max_entries`: Capacity of `out_ptr`, in entries (not bytes).
///
/// Returns the *total* number of currently registered drivers, which may
/// exceed `max_entries` — the caller can compare the two to detect
/// truncation. Exactly `min(total, max_entries)` entries are written.
/// `registry::MAX_DRIVERS` bounds the total tightly enough in practice
/// (16 today) that a caller sizing its buffer to that constant never
/// observes truncation.
///
/// A single registry-lock acquisition produces the whole snapshot, unlike
/// the earlier count-then-fetch-by-index pair this syscall replaces: there
/// is no window in which a driver registering or exiting between two calls
/// could shift what an index refers to.
///
/// Callable by any task — enumerating driver names/task-ids is not itself a
/// privileged operation, mirroring `DrvLookup`.
pub fn syscall_drv_list_impl(out_ptr: *mut UserDriverInfo, max_entries: u64) -> SyscallResult<u64> {
    // Compile-time guard: `UserDriverInfo::name` and `DriverEntry::name` must
    // stay the same size, since `entry.name` below is copied into it directly.
    const _: () = assert!(
        registry::DRIVER_NAME_LEN == crate::syscall::types::USER_DRIVER_NAME_LEN,
        "UserDriverInfo::name and registry::DriverEntry::name must have matching lengths"
    );

    // Step 1: snapshot the registry once. `n` is bounded by the registry's
    // own fixed capacity (`registry::MAX_DRIVERS`, 16 today) regardless of
    // what `max_entries` the caller passes, so `n * size_of::<UserDriverInfo>()`
    // below can never overflow even for a caller-supplied `max_entries` of
    // `u64::MAX`.
    let entries = registry::list();
    let total = entries.len() as u64;
    let n = (max_entries as usize).min(entries.len());

    if n == 0 {
        return Ok(total);
    }

    // Step 2: validate alignment and writability of the destination buffer,
    // mirroring `syscall_get_pci_device_impl`'s validation of `UserPciDevice`.
    if !(out_ptr as u64).is_multiple_of(core::mem::align_of::<UserDriverInfo>() as u64) {
        return Err(SyscallError::InvalidArg);
    }
    if !is_valid_user_buffer_writable(
        out_ptr as *const u8,
        n * core::mem::size_of::<UserDriverInfo>(),
    ) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 3: copy the snapshot out, entry by entry.
    for (i, entry) in entries.iter().take(n).enumerate() {
        let info = UserDriverInfo {
            name: entry.name,
            name_len: entry.name_len as u32,
            _padding: 0,
            tid: entry.tid as u64,
        };
        // SAFETY:
        // - Alignment and writability of `out_ptr` for `n` entries were
        //   verified in Step 2, and `i < n`.
        // - `write_unaligned` is safe for any alignment (redundant with the
        //   alignment check above, kept for defense in depth like `DrvQuery`).
        unsafe {
            core::ptr::write_unaligned(out_ptr.add(i), info);
        }
    }

    Ok(total)
}

/// Checks whether a driver binary is known to the kernel and currently has
/// a matching PCI device present, without any of `SpawnDriver`'s side
/// effects (no device reservation, no Command Register writes).
///
/// This lets a caller — concretely, `DRIVERS.BIN`'s `load` command — print
/// a specific "unknown driver" vs. "no matching device" error *before*
/// attempting to spawn anything, without hand-duplicating the kernel's own
/// binary-name-to-PCI-ID table (`driver_db::DRIVER_DB`) client-side the way
/// an earlier version of this codebase did.
///
/// Arguments:
/// - `name_ptr`: Pointer to a NUL-terminated driver binary filename in user
///   memory (mirrors `SpawnDriver`'s own `name_ptr` argument).
///
/// Returns `Ok(1)` if `name` is a known driver and a matching device is
/// currently present, `Ok(0)` if `name` is known but no matching device is
/// present, or `Err(InvalidArg)` if `name` is not a known driver binary at
/// all.
///
/// Callable by any task — a read-only query over the kernel's own
/// compile-time driver table and its cached PCI enumeration is not a
/// privileged operation, mirroring `DrvLookup`.
pub fn syscall_drv_probe_impl(name_ptr: *const u8) -> SyscallResult<u64> {
    use crate::drivers::driver_db;

    let name = super::fs::read_user_string(name_ptr, 128)?;
    let present = driver_db::device_present(&name).map_err(|_| SyscallError::InvalidArg)?;
    Ok(present as u64)
}
