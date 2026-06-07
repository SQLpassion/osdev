//! Kernel-side syscall dispatcher (`int 0x80` path).
//!
//! Responsibilities of this module:
//! - decode syscall number + ABI arguments,
//! - route to the corresponding kernel implementation,
//! - enforce minimal argument validation at syscall boundaries,
//! - return stable numeric result/error codes to caller context.
//!
//! ABI for `dispatch` (provided by interrupt entry glue):
//! - `RAX` -> `syscall_nr`
//! - `RDI` -> `arg0`
//! - `RSI` -> `arg1`
//! - `RDX` -> `arg2`
//! - `R10` -> `arg3`

use core::slice;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::keyboard;
use crate::drivers::screen::with_screen;
use crate::drivers::serial::Serial;
use crate::logging;
use crate::scheduler;

use super::{
    is_valid_user_buffer, syscall_result_to_raw, SyscallError, SyscallId, SyscallResult, SYSCALL_OK,
};

/// Maximum number of bytes that can be written in a single WriteSerial syscall.
/// This limit prevents denial-of-service by bounding syscall execution time
/// and ensures fair CPU scheduling among tasks.
const MAX_SERIAL_WRITE_LEN: usize = 4096;
/// Maximum number of bytes that can be written in a single WriteConsole syscall.
/// Same DoS/fairness rationale as `MAX_SERIAL_WRITE_LEN`.
const MAX_CONSOLE_WRITE_LEN: usize = 4096;

/// Global switch for per-syscall trace logging (`[SYSCALL] ...` lines).
static SYSCALL_TRACE_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable/disable syscall trace logging.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_syscall_trace_enabled(enabled: bool) {
    SYSCALL_TRACE_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Returns whether syscall trace logging is currently enabled.
pub fn syscall_trace_enabled() -> bool {
    SYSCALL_TRACE_ENABLED.load(Ordering::Relaxed)
}

/// Returns the stable human-readable syscall name for a raw syscall number.
///
/// Used by dispatcher logging so serial traces remain understandable without
/// requiring external number-to-name lookup tables.
pub const fn syscall_name_for_number(syscall_nr: u64) -> &'static str {
    match syscall_nr {
        SyscallId::YIELD => "Yield",
        SyscallId::WRITE_SERIAL => "WriteSerial",
        SyscallId::EXIT => "Exit",
        SyscallId::WRITE_CONSOLE => "WriteConsole",
        SyscallId::GET_CHAR => "GetChar",
        SyscallId::GET_CURSOR => "GetCursor",
        SyscallId::SET_CURSOR => "SetCursor",
        SyscallId::CLEAR_SCREEN => "ClearScreen",
        SyscallId::OPEN_FILE => "OpenFile",
        SyscallId::CLOSE_FILE => "CloseFile",
        SyscallId::READ_FILE => "ReadFile",
        SyscallId::WRITE_FILE => "WriteFile",
        SyscallId::DELETE_FILE => "DeleteFile",
        SyscallId::SEEK_FILE => "SeekFile",
        SyscallId::END_OF_FILE => "EndOfFile",
        SyscallId::PRINT_ROOT_DIRECTORY => "PrintRootDirectory",
        SyscallId::MMAP => "Mmap",
        _ => "Unknown",
    }
}

/// Resolves syscall number and dispatches to the corresponding kernel handler.
///
/// ABI contract (as set by `int 0x80` entry glue):
/// - `syscall_nr`: `RAX`
/// - `arg0..arg3`: `RDI`, `RSI`, `RDX`, `R10`
///
/// Returns kernel-internal typed results.
///
/// Raw ABI conversion to sentinel `u64` values is done at the syscall boundary.
pub fn dispatch_checked(
    syscall_nr: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
) -> SyscallResult<u64> {
    // Step 1: resolve the handler and compute the syscall return value.
    let result = match syscall_nr {
        SyscallId::YIELD => syscall_yield_impl(),
        SyscallId::WRITE_SERIAL => syscall_write_serial_impl(arg0 as *const u8, arg1 as usize),
        SyscallId::WRITE_CONSOLE => syscall_write_console_impl(arg0 as *const u8, arg1 as usize),
        SyscallId::GET_CHAR => syscall_getchar_impl(),
        SyscallId::GET_CURSOR => syscall_get_cursor_impl(),
        SyscallId::SET_CURSOR => syscall_set_cursor_impl(arg0 as usize, arg1 as usize),
        SyscallId::CLEAR_SCREEN => syscall_clear_screen_impl(),
        SyscallId::EXIT => syscall_exit_impl(),
        SyscallId::OPEN_FILE => syscall_open_file_impl(arg0 as *const u8, arg1),
        SyscallId::CLOSE_FILE => syscall_close_file_impl(arg0),
        SyscallId::READ_FILE => syscall_read_file_impl(arg0, arg1 as *mut u8, arg2),
        SyscallId::WRITE_FILE => syscall_write_file_impl(arg0, arg1 as *const u8, arg2),
        SyscallId::DELETE_FILE => syscall_delete_file_impl(arg0 as *const u8),
        SyscallId::SEEK_FILE => syscall_seek_file_impl(arg0, arg1),
        SyscallId::END_OF_FILE => syscall_end_of_file_impl(arg0),
        SyscallId::PRINT_ROOT_DIRECTORY => syscall_print_root_directory_impl(),
        SyscallId::MMAP => syscall_mmap_impl(arg0, arg1 as usize),
        _ => Err(SyscallError::Unsupported),
    };

    // Step 2: emit one serial trace line for every syscall dispatch.
    // This gives deterministic kernel-side visibility into syscall traffic.
    let raw_result = syscall_result_to_raw(result);
    let trace_enabled = syscall_trace_enabled();
    logging::logln_with_options(
        "syscall",
        format_args!(
            "[SYSCALL] nr={} name={} arg0={:#x} arg1={:#x} arg2={:#x} arg3={:#x} ret={:#x}",
            syscall_nr,
            syscall_name_for_number(syscall_nr),
            arg0,
            arg1,
            arg2,
            arg3,
            raw_result
        ),
        trace_enabled,
        trace_enabled,
    );

    result
}

/// ABI-compatible raw dispatcher (`Result` encoded to sentinel `u64` values).
#[cfg_attr(not(test), allow(dead_code))]
pub fn dispatch(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    syscall_result_to_raw(dispatch_checked(syscall_nr, arg0, arg1, arg2, arg3))
}

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
fn syscall_yield_impl() -> SyscallResult<u64> {
    Ok(SYSCALL_OK)
}

/// Implements `WriteSerial(ptr, len)`.
///
/// Writes up to `MAX_SERIAL_WRITE_LEN` bytes from the user buffer to COM1.
/// If the requested length exceeds the maximum, only the first
/// `MAX_SERIAL_WRITE_LEN` bytes are written.
///
/// Behavior:
/// - `len == 0` is treated as success and returns `0`,
/// - null pointer with non-zero `len` returns `SYSCALL_ERR_INVALID_ARG`,
/// - claimed `(ptr, len)` that overflows, crosses the canonical boundary,
///   or reaches into kernel space returns `SYSCALL_ERR_INVALID_ARG`,
/// - otherwise bytes are read from caller memory and written to COM1,
///   returning the number of bytes actually written (≤ `MAX_SERIAL_WRITE_LEN`).
///
/// # Validation order
/// The full claimed range `ptr..ptr+len` is validated before any clamping so
/// that callers whose buffer description is structurally invalid (overflowing
/// end address, boundary-crossing, kernel pointer) always receive `EINVAL`
/// rather than a silent partial success.
///
/// # DoS Protection
/// The maximum write length prevents a single syscall from monopolizing
/// the CPU for an unbounded duration. User code must chunk large writes
/// into multiple syscalls.
fn syscall_write_serial_impl(ptr: *const u8, len: usize) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }

    // Step 1: Validate the full claimed range before clamping.
    // A caller whose (ptr, len) overflows, crosses the canonical boundary,
    // is null, or reaches into kernel space receives EINVAL — even though
    // we will subsequently write fewer than `len` bytes due to the DoS cap.
    if !is_valid_user_buffer(ptr, len) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Clamp to maximum to prevent denial-of-service.
    // The full range was validated above; the slice covers a valid sub-range.
    let actual_len = len.min(MAX_SERIAL_WRITE_LEN);

    let bytes = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it builds a slice from a raw userspace pointer.
        // - `is_valid_user_buffer` above verified that `ptr..ptr+len` lies
        //   entirely within user canonical space.
        // - `actual_len <= len`, so `ptr..ptr+actual_len` is a valid sub-range.
        // - `actual_len` is bounded by `MAX_SERIAL_WRITE_LEN`.
        slice::from_raw_parts(ptr, actual_len)
    };

    let serial = Serial::new();

    for byte in bytes {
        serial.write_byte(*byte);
    }

    Ok(actual_len as u64)
}

/// Implements `WriteConsole(ptr, len)`.
///
/// Writes up to `MAX_CONSOLE_WRITE_LEN` bytes from the user buffer to the VGA
/// text console. Semantics mirror `WriteSerial`:
/// - `len == 0` returns `0`,
/// - claimed `(ptr, len)` that overflows, crosses the canonical boundary,
///   or reaches into kernel space returns `SYSCALL_ERR_INVALID_ARG`,
/// - successful call returns number of bytes written (≤ `MAX_CONSOLE_WRITE_LEN`).
///
/// Bytes are written as raw VGA text characters; this syscall does not enforce
/// UTF-8 validity and is intended for simple ASCII/debug output.
///
/// # Validation order
/// The full claimed range `ptr..ptr+len` is validated before any clamping —
/// mirroring the `WriteSerial` contract exactly.
fn syscall_write_console_impl(ptr: *const u8, len: usize) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }

    // Step 1: Validate the full claimed range before clamping.
    if !is_valid_user_buffer(ptr, len) {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Clamp to maximum to prevent denial-of-service.
    let actual_len = len.min(MAX_CONSOLE_WRITE_LEN);

    let bytes = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it builds a slice from a raw userspace pointer.
        // - `is_valid_user_buffer` above verified that `ptr..ptr+len` lies
        //   entirely within user canonical space.
        // - `actual_len <= len`, so `ptr..ptr+actual_len` is a valid sub-range.
        // - `actual_len` is bounded by `MAX_CONSOLE_WRITE_LEN`.
        slice::from_raw_parts(ptr, actual_len)
    };

    with_screen(|screen| {
        for byte in bytes {
            screen.print_char(*byte);
        }
    });

    Ok(actual_len as u64)
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
fn syscall_getchar_impl() -> SyscallResult<u64> {
    Ok(keyboard::read_char_blocking() as u64)
}

/// Implements `GetCursor()`.
///
/// Returns the current VGA cursor as a packed 64-bit value:
/// - upper 32 bits: `row`
/// - lower 32 bits: `col`
fn syscall_get_cursor_impl() -> SyscallResult<u64> {
    Ok(with_screen(|screen| {
        let (row, col) = screen.get_cursor();
        ((row as u64) << 32) | (col as u64)
    }))
}

/// Implements `SetCursor(row, col)`.
///
/// Sets the VGA cursor position. Values outside the current screen bounds are
/// clamped by the screen driver.
fn syscall_set_cursor_impl(row: usize, col: usize) -> SyscallResult<u64> {
    with_screen(|screen| {
        screen.set_cursor(row, col);
    });
    Ok(SYSCALL_OK)
}

/// Implements `ClearScreen()`.
///
/// Clears the entire VGA text buffer and resets the cursor position to `(0, 0)`.
fn syscall_clear_screen_impl() -> SyscallResult<u64> {
    with_screen(|screen| {
        screen.clear();
    });
    Ok(SYSCALL_OK)
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
fn syscall_exit_impl() -> SyscallResult<u64> {
    scheduler::mark_current_as_zombie();
    Ok(SYSCALL_OK)
}

/// Helper to read a null-terminated string from user space safely.
fn read_user_string(ptr: *const u8, max_len: usize) -> Result<alloc::string::String, SyscallError> {
    if ptr.is_null() {
        return Err(SyscallError::InvalidArg);
    }

    let mut len = 0;
    loop {
        if len >= max_len {
            return Err(SyscallError::InvalidArg);
        }

        if !is_valid_user_buffer(unsafe { ptr.add(len) }, 1) {
            return Err(SyscallError::InvalidArg);
        }

        // SAFETY:
        // - We validated the pointer points to a valid user memory buffer.
        let b = unsafe { *ptr.add(len) };
        if b == 0 {
            break;
        }
        len += 1;
    }

    // SAFETY:
    // - We validated the memory range from ptr to ptr + len is valid user space.
    let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    let s = core::str::from_utf8(slice).map_err(|_| SyscallError::InvalidArg)?;
    Ok(alloc::string::String::from(s))
}

/// Implements `OpenFile()`.
fn syscall_open_file_impl(name_ptr: *const u8, mode_raw: u64) -> SyscallResult<u64> {
    let name = read_user_string(name_ptr, 128)?;
    let mode = match mode_raw {
        0 => crate::io::fat12::FileMode::Read,
        1 => crate::io::fat12::FileMode::Write,
        2 => crate::io::fat12::FileMode::Append,
        _ => return Err(SyscallError::InvalidArg),
    };

    let fd = crate::io::fat12::open_file(&name, mode).map_err(|_| SyscallError::Io)?;
    Ok(fd as u64)
}

/// Implements `CloseFile()`.
fn syscall_close_file_impl(fd: u64) -> SyscallResult<u64> {
    crate::io::fat12::close_file(fd as usize).map_err(|_| SyscallError::InvalidArg)?;
    Ok(0)
}

/// Implements `ReadFile()`.
fn syscall_read_file_impl(fd: u64, buf_ptr: *mut u8, len: u64) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }
    if !is_valid_user_buffer(buf_ptr, len as usize) {
        return Err(SyscallError::InvalidArg);
    }

    // SAFETY:
    // - We validated the buffer is a valid user memory range.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len as usize) };
    let bytes_read =
        crate::io::fat12::read_file_fd(fd as usize, buffer).map_err(|_| SyscallError::Io)?;
    Ok(bytes_read as u64)
}

/// Implements `WriteFile()`.
fn syscall_write_file_impl(fd: u64, buf_ptr: *const u8, len: u64) -> SyscallResult<u64> {
    if len == 0 {
        return Ok(0);
    }
    if !is_valid_user_buffer(buf_ptr, len as usize) {
        return Err(SyscallError::InvalidArg);
    }

    // SAFETY:
    // - We validated the buffer is a valid user memory range.
    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, len as usize) };
    let bytes_written =
        crate::io::fat12::write_file_fd(fd as usize, buffer).map_err(|_| SyscallError::Io)?;
    Ok(bytes_written as u64)
}

/// Implements `DeleteFile()`.
fn syscall_delete_file_impl(name_ptr: *const u8) -> SyscallResult<u64> {
    let name = read_user_string(name_ptr, 128)?;
    crate::io::fat12::delete_file(&name).map_err(|_| SyscallError::Io)?;
    Ok(0)
}

/// Implements `SeekFile()`.
fn syscall_seek_file_impl(fd: u64, offset: u64) -> SyscallResult<u64> {
    crate::io::fat12::seek_file(fd as usize, offset as u32)
        .map_err(|_| SyscallError::InvalidArg)?;
    Ok(0)
}

/// Implements `EndOfFile()`.
fn syscall_end_of_file_impl(fd: u64) -> SyscallResult<u64> {
    let eof = crate::io::fat12::eof_file(fd as usize).map_err(|_| SyscallError::InvalidArg)?;
    Ok(if eof { 1 } else { 0 })
}

/// Implements `PrintRootDirectory()`.
fn syscall_print_root_directory_impl() -> SyscallResult<u64> {
    crate::io::fat12::print_root_directory();
    Ok(0)
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
fn syscall_mmap_impl(addr: u64, length: usize) -> SyscallResult<u64> {
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
