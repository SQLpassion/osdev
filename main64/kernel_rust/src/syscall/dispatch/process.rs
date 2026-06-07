//! Process-related system call implementations (Yield, Exit, GetChar, Mmap).

use crate::drivers::keyboard;
use crate::scheduler;
use crate::syscall::types::{SyscallResult, SyscallError, SYSCALL_OK};

/// Implements `Yield`: cooperative handoff to scheduler.
///
/// This function only returns the result code — it does **not** trigger the
/// reschedule itself.  The actual context switch is performed by the caller
/// [`syscall_rust_dispatch`](crate::arch::interrupts::syscall_rust_dispatch),
/// which calls [`on_timer_tick`](crate::scheduler::on_timer_tick) directly
/// with the current interrupt frame after `dispatch` returns.
///
/// # Why not call `yield_now()` here?
///
/// `yield_now()` issues `int 32` (PIT timer vector) to enter the scheduler.
/// When called from inside the `int 0x80` handler, this would create a
/// **nested interrupt**: the CPU pushes a second IRET frame and a second
/// register save onto the same kernel stack.  This has three problems:
///
/// 1. **Double stack consumption** — two full register saves plus two IRET
///    frames (~320 bytes) per yield, eating into the 64 KiB task kernel stack.
/// 2. **Unnecessary overhead** — two interrupt entry/exit round-trips instead
///    of one.
/// 3. **Fragility** — the scheduler sees the inner `int 32` frame rather than
///    the original `int 0x80` frame that holds the actual user-mode context.
///
/// By returning `SYSCALL_OK` here and letting `syscall_rust_dispatch` feed
/// the *original* `int 0x80` frame into `on_timer_tick`, the scheduler sees
/// the correct user context and can switch tasks with a single `iretq`.
pub fn syscall_yield_impl() -> SyscallResult<u64> {
    Ok(SYSCALL_OK)
}

/// Implements `GetChar()`.
///
/// Reads a single character from the keyboard, blocking the calling task
/// until input becomes available. This syscall mirrors the C kernel's
/// `SYSCALL_GETCHAR` behavior.
///
/// The keyboard driver maintains a decoded character buffer that is populated
/// by a dedicated keyboard worker task. When the buffer is empty, this syscall
/// puts the calling task to sleep on the input wait queue. The keyboard worker
/// wakes waiting tasks once it has decoded new input.
///
/// # Blocking Behavior
/// This syscall **always blocks** until a character is available. The task is
/// rescheduled by the normal scheduler flow when woken by the keyboard worker.
///
/// # Return Value
/// Returns the ASCII value of the decoded character (0-255). Special keys that
/// don't produce printable characters are filtered out by the keyboard driver.
pub fn syscall_getchar_impl() -> SyscallResult<u64> {
    Ok(keyboard::read_char_blocking() as u64)
}

/// Implements `Exit()`.
///
/// Marks the current task as [`Zombie`](crate::scheduler::TaskState::Zombie)
/// and returns `SYSCALL_OK`. The actual reschedule is driven by
/// [`syscall_rust_dispatch`](crate::arch::interrupts::syscall_rust_dispatch),
/// which calls [`on_timer_tick`](crate::scheduler::on_timer_tick) directly —
/// analogous to the Yield path.
///
/// The zombie task will never be selected again and is reaped on the
/// following scheduler tick once execution has moved off its kernel stack.
///
/// # Exit Code
/// This syscall does not accept an exit code parameter. If future support
/// for process wait semantics is added, the exit code parameter can be
/// reintroduced and stored in the task entry for retrieval by a parent task.
pub fn syscall_exit_impl() -> SyscallResult<u64> {
    scheduler::mark_current_as_zombie();
    Ok(SYSCALL_OK)
}

/// Implements `Mmap(addr, length)`: dynamically allocate physical frames and map them
/// into user space.
///
/// # Arguments (ABI)
/// - `addr` (`arg0` / `RDI`): target virtual address for the mapping.  When zero the
///   kernel picks the next contiguous heap address automatically.  When non-zero the
///   value must be page-aligned and equal to the current heap top — this enforces
///   strictly contiguous growth and prevents holes.
/// - `length` (`arg1` / `RSI`): requested mapping size in bytes (rounded up to page
///   granularity by the kernel).
///
/// Under the hood, this grows the user-space heap region of the calling task dynamically.
/// Memory pages are mapped as writable, non-executable, and user-accessible.
///
/// # Invariants and Safety
/// - No unsafe blocks are directly executed in this function (page table and register
///   writes are encapsulated within safe-looking PMM/VMM/scheduler APIs).
/// - Rollback safety: if any page fails to map (e.g. page directory allocation fails),
///   all pages mapped during this call are rolled back, and all allocated physical frames
///   are released back to the PMM to prevent memory leaks.
pub fn syscall_mmap_impl(addr: u64, length: usize) -> SyscallResult<u64> {
    // Step 1: Reject zero-length allocations immediately.
    if length == 0 {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Fetch the active task ID from the scheduler.
    let task_id = scheduler::current_task_id().ok_or(SyscallError::InvalidArg)?;

    // Step 3: Extract the PML4 address space root (CR3) for the current task.
    let (cr3, _, _) = scheduler::task_context(task_id).ok_or(SyscallError::InvalidArg)?;

    // Step 4: Retrieve the current heap boundary (brk) for the active task.
    let current_heap_top = scheduler::current_user_heap_top().ok_or(SyscallError::InvalidArg)?;

    // Step 4b: Validate the requested target address.
    // When `addr` is non-zero the caller is requesting a specific VA.  We enforce:
    //   (a) page alignment,
    //   (b) the address falls within the user heap window,
    //   (c) it matches the current heap top — ensuring strictly contiguous growth.
    // When `addr` is zero the kernel silently picks `current_heap_top` (backward
    // compatible behaviour).
    if addr != 0 {
        let page_mask = crate::arch::constants::PAGE_SIZE_U64 - 1;
        if addr & page_mask != 0 {
            return Err(SyscallError::InvalidArg);
        }
        if !(crate::memory::vmm::USER_HEAP_BASE..crate::memory::vmm::USER_HEAP_END).contains(&addr)
        {
            return Err(SyscallError::InvalidArg);
        }
        if addr != current_heap_top {
            return Err(SyscallError::InvalidArg);
        }
    }

    // Step 5: Align the requested length up to a multiple of PAGE_SIZE_U64 (4 KiB).
    let page_size = crate::arch::constants::PAGE_SIZE_U64;
    let aligned_len = match (length as u64).checked_add(page_size - 1) {
        Some(sum) => sum & !(page_size - 1),
        None => return Err(SyscallError::InvalidArg),
    };

    // Step 6: Verify the allocation does not overflow or exceed the maximum heap window boundary.
    let new_heap_top = match current_heap_top.checked_add(aligned_len) {
        Some(top) if top <= crate::memory::vmm::USER_HEAP_END => top,
        _ => return Err(SyscallError::InvalidArg),
    };

    let num_pages = aligned_len / page_size;

    // Step 7: Allocate the physical memory frames from the PMM.
    // We pre-reserve Vec capacity so that vector pushes are infallible.
    let mut allocated_pfns = alloc::vec::Vec::new();
    if allocated_pfns.try_reserve(num_pages as usize).is_err() {
        return Err(SyscallError::OutOfMemory);
    }

    for _ in 0..num_pages {
        if let Some(frame) = crate::memory::pmm::with_pmm(|mgr| mgr.alloc_frame()) {
            allocated_pfns.push(frame.pfn);
        } else {
            // Rollback physical frame allocations on failure.
            for pfn in allocated_pfns {
                crate::memory::pmm::with_pmm(|mgr| mgr.release_pfn(pfn));
            }
            return Err(SyscallError::OutOfMemory);
        }
    }

    // Step 8: Map the allocated physical frames into the task's address space.
    // All VMM page mapping is performed under `with_address_space` to guarantee active page directory context.
    let mut map_failed = false;
    let mut mapped_count = 0;

    crate::memory::vmm::with_address_space(cr3, || {
        for (i, &pfn) in allocated_pfns.iter().enumerate() {
            let page_va = current_heap_top + (i as u64 * page_size);
            if crate::memory::vmm::map_user_page(page_va, pfn, true).is_err() {
                map_failed = true;
                break;
            }
            mapped_count += 1;
        }

        // If mapping failed mid-way, rollback all pages mapped during this call.
        if map_failed {
            for j in 0..mapped_count {
                let roll_va = current_heap_top + (j as u64 * page_size);
                crate::memory::vmm::unmap_virtual_address(roll_va);
            }
        }
    });

    // Step 9: Release frames that were not successfully mapped (or were rolled back).
    if map_failed {
        for &pfn in &allocated_pfns[mapped_count..] {
            crate::memory::pmm::with_pmm(|mgr| mgr.release_pfn(pfn));
        }
        return Err(SyscallError::Io);
    }

    // Step 10: Commit the new heap top to the scheduler metadata and return the start address of the block.
    if !scheduler::set_current_user_heap_top(new_heap_top) {
        // Rollback all mapped pages if scheduler update fails (highly unlikely).
        crate::memory::vmm::with_address_space(cr3, || {
            for j in 0..num_pages {
                let roll_va = current_heap_top + (j * page_size);
                crate::memory::vmm::unmap_virtual_address(roll_va);
            }
        });
        return Err(SyscallError::Io);
    }

    Ok(current_heap_top)
}

/// Implements `Exec(name_ptr)`: load and spawn a new user task from FAT12.
///
/// Reads the name string from user space safely and invokes the file loader.
/// Each `ExecError` variant is mapped to the most appropriate `SyscallError`
/// so user-space callers can distinguish file-not-found from spawn failures.
pub fn syscall_exec_impl(name_ptr: *const u8) -> SyscallResult<u64> {
    use crate::process::ExecError;

    // Step 1: Decode the null-terminated string from user-space memory.
    let name = super::fs::read_user_string(name_ptr, 128)?;

    // Step 2: Attempt to load the program image and spawn a user task.
    let result = crate::process::exec_from_fat12(&name);

    // Step 3: Log the outcome to serial so failures are visible without a debugger.
    match &result {
        Ok(tid) => {
            crate::logging::logln(
                "exec",
                format_args!("EXEC: spawned '{}' as task {}", name, tid),
            );
        }
        Err(e) => {
            crate::logging::logln(
                "exec",
                format_args!("EXEC: failed to exec '{}': {:?}", name, e),
            );
        }
    }

    // Step 4: Map ExecError variants to distinct SyscallError codes.
    result.map(|tid| tid as u64).map_err(|e| match e {
        ExecError::InvalidName => SyscallError::InvalidArg,
        ExecError::NotFound => SyscallError::InvalidArg,
        ExecError::IsDirectory => SyscallError::InvalidArg,
        ExecError::EmptyImage => SyscallError::InvalidArg,
        ExecError::FileTooLarge => SyscallError::InvalidArg,
        ExecError::OutOfMemory => SyscallError::OutOfMemory,
        ExecError::MappingFailed => SyscallError::OutOfMemory,
        ExecError::SpawnFailed => SyscallError::Io,
        ExecError::Io => SyscallError::Io,
    })
}

/// Implements `Wait(task_id)`: block current task until target task exits.
pub fn syscall_wait_impl(task_id: u64) -> SyscallResult<u64> {
    // Step 1: Delegate task wait to the scheduler exit queue.
    scheduler::wait_for_task_exit(task_id as usize);

    Ok(SYSCALL_OK)
}

/// Implements `Shutdown()`: shuts down the virtual machine.
pub fn syscall_shutdown_impl() -> SyscallResult<u64> {
    // Step 1: Trigger system shutdown.
    crate::arch::power::shutdown();
}

