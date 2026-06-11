//! Ring-3 cursor syscall demo (`cursordemo` command).
//!
//! Purpose:
//! - exercise `SetCursor`/`GetCursor` through the real user-mode `int 0x80` path,
//! - validate expected cursor behavior (roundtrip, clamping, newline transition),
//! - print explicit PASS/FAIL markers to VGA so behavior is visible without debugger.
//! - start with a clean screen and terminate only after ESC key press.
//!
//! Execution model:
//! 1. Clone kernel page tables into a dedicated user CR3.
//! 2. Map required code pages for this task and syscall wrappers into user code window.
//! 3. Map one user stack page and one user data page.
//! 4. Copy all demo message strings into the user data page.
//! 5. Spawn ring-3 task and wait until it exits.

use crate::drivers::screen::with_screen;
use crate::memory::pmm;
use crate::memory::vmm;
use crate::scheduler;
use crate::syscall;
use alloc::vec::Vec;
use core::fmt::Write;

/// User stack page VA (mapped writable).
const USER_CURSOR_TASK_STACK_PAGE_VA: u64 = vmm::USER_STACK_TOP - 0x1000;

/// Initial user stack pointer (16-byte aligned, top of mapped stack page).
const USER_CURSOR_TASK_STACK_TOP: u64 = vmm::USER_STACK_TOP - 16;

/// User data page VA that stores readonly message bytes for ring-3 output.
const USER_CURSOR_TASK_DATA_VA: u64 = vmm::USER_STACK_TOP - 0x2000;

/// Number of adjacent code pages mapped per function symbol.
///
/// Rationale:
/// Some debug builds place `cursor_ring3_task` across multiple pages.
/// Mapping four pages per symbol avoids latent user-mode instruction-fetch faults
/// when control flow crosses beyond the first two pages.
const USER_CURSOR_CODE_PAGES_PER_SYMBOL: u64 = 4;

// Demo output messages copied into the user data page.
const CURSOR_MSG_START: &[u8] = b"[ring3] Cursor demo running\n";
const CURSOR_MSG_PASS_ROUNDTRIP: &[u8] = b"[PASS] SetCursor/GetCursor roundtrip\n";
const CURSOR_MSG_FAIL_ROUNDTRIP: &[u8] = b"[FAIL] SetCursor/GetCursor roundtrip\n";
const CURSOR_MSG_PASS_CLAMP: &[u8] = b"[PASS] SetCursor clamp to screen bounds\n";
const CURSOR_MSG_FAIL_CLAMP: &[u8] = b"[FAIL] SetCursor clamp to screen bounds\n";
const CURSOR_MSG_PASS_NL: &[u8] = b"[PASS] newline cursor transition\n";
const CURSOR_MSG_FAIL_NL: &[u8] = b"[FAIL] newline cursor transition\n";
const CURSOR_MSG_SUMMARY_OK: &[u8] = b"[ring3] Cursor demo complete: PASS\n";
const CURSOR_MSG_SUMMARY_FAIL: &[u8] = b"[ring3] Cursor demo complete: FAIL\n";
const CURSOR_MSG_ERR: &[u8] = b"[ring3] Cursor syscall failed\n";
const CURSOR_MSG_WAIT_ESC: &[u8] = b"[ring3] Press ESC to clear screen and exit cursor demo\n";
const CURSOR_MSG_EXIT: &[u8] = b"[ring3] ESC pressed. Screen cleared. Exiting cursor demo.\n";
const CURSOR_MSG_NL_ONLY: &[u8] = b"\n";

// Compact string table layout inside USER_CURSOR_TASK_DATA_VA page.
// Offsets are computed statically to avoid runtime allocations/formatting.
const CURSOR_MSG_START_OFFSET: usize = 0;
const CURSOR_MSG_PASS_ROUNDTRIP_OFFSET: usize = CURSOR_MSG_START_OFFSET + CURSOR_MSG_START.len();
const CURSOR_MSG_FAIL_ROUNDTRIP_OFFSET: usize =
    CURSOR_MSG_PASS_ROUNDTRIP_OFFSET + CURSOR_MSG_PASS_ROUNDTRIP.len();
const CURSOR_MSG_PASS_CLAMP_OFFSET: usize =
    CURSOR_MSG_FAIL_ROUNDTRIP_OFFSET + CURSOR_MSG_FAIL_ROUNDTRIP.len();
const CURSOR_MSG_FAIL_CLAMP_OFFSET: usize =
    CURSOR_MSG_PASS_CLAMP_OFFSET + CURSOR_MSG_PASS_CLAMP.len();
const CURSOR_MSG_PASS_NL_OFFSET: usize = CURSOR_MSG_FAIL_CLAMP_OFFSET + CURSOR_MSG_FAIL_CLAMP.len();
const CURSOR_MSG_FAIL_NL_OFFSET: usize = CURSOR_MSG_PASS_NL_OFFSET + CURSOR_MSG_PASS_NL.len();
const CURSOR_MSG_SUMMARY_OK_OFFSET: usize = CURSOR_MSG_FAIL_NL_OFFSET + CURSOR_MSG_FAIL_NL.len();
const CURSOR_MSG_SUMMARY_FAIL_OFFSET: usize =
    CURSOR_MSG_SUMMARY_OK_OFFSET + CURSOR_MSG_SUMMARY_OK.len();
const CURSOR_MSG_ERR_OFFSET: usize = CURSOR_MSG_SUMMARY_FAIL_OFFSET + CURSOR_MSG_SUMMARY_FAIL.len();
const CURSOR_MSG_WAIT_ESC_OFFSET: usize = CURSOR_MSG_ERR_OFFSET + CURSOR_MSG_ERR.len();
const CURSOR_MSG_EXIT_OFFSET: usize = CURSOR_MSG_WAIT_ESC_OFFSET + CURSOR_MSG_WAIT_ESC.len();
const CURSOR_MSG_NL_ONLY_OFFSET: usize = CURSOR_MSG_EXIT_OFFSET + CURSOR_MSG_EXIT.len();

/// Launches the ring-3 cursor demo task and blocks until completion.
///
/// Failure handling:
/// - on setup/mapping failures, prints an error and tears down user address space,
/// - on successful spawn, waits until the task has exited.
pub(crate) fn run_user_mode_cursor_demo() {
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    if let Err(msg) = map_cursor_task_pages(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "Cursor demo setup failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    if let Err(msg) = write_cursor_data_page(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "Cursor demo data write failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    let entry_kernel_va = cursor_ring3_task as *const () as usize as u64;
    let entry_rip = match crate::kernel_va_to_user_code_va(entry_kernel_va) {
        Some(va) => va,
        None => {
            with_screen(|screen| {
                writeln!(
                    screen,
                    "Cursor demo spawn failed: entry address outside user code alias window"
                )
                .unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    let task_id = match scheduler::spawn_user_task(entry_rip, USER_CURSOR_TASK_STACK_TOP, user_cr3)
    {
        Ok(task_id) => task_id,
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "Cursor demo spawn failed: {:?}", err).unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    // Step 6: Ensure round-robin execution is active before waiting.
    // In normal boot flow the scheduler is already started in `KernelMain`.
    // Integration tests may call this demo with an initialized-but-not-started
    // scheduler, which would make `yield_now()` a no-op and stall this loop.
    if !scheduler::is_running() {
        scheduler::start();
    }

    while scheduler::task_frame_ptr(task_id).is_some() {
        scheduler::yield_now();
    }
}

/// Maps all virtual memory required by the ring-3 cursor demo task.
///
/// Mapped regions:
/// - user-code alias pages for task entry + syscall wrappers/ABI helpers,
/// - one writable user stack page,
/// - one readonly user data page (populated after mapping).
fn map_cursor_task_pages(target_cr3: u64) -> Result<(), &'static str> {
    let required_kernel_function_vas: [u64; 10] = [
        cursor_ring3_task as *const () as usize as u64,
        syscall::user::sys_get_cursor as *const () as usize as u64,
        syscall::user::sys_set_cursor as *const () as usize as u64,
        syscall::user::sys_clear_screen as *const () as usize as u64,
        syscall::user::sys_getchar as *const () as usize as u64,
        syscall::user::sys_write_console as *const () as usize as u64,
        syscall::user::sys_exit as *const () as usize as u64,
        syscall::abi::syscall0 as *const () as usize as u64,
        syscall::abi::syscall1 as *const () as usize as u64,
        syscall::abi::syscall2 as *const () as usize as u64,
    ];

    let stack_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user cursor task stack frame")
    });
    let data_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user cursor task data frame")
    });

    vmm::with_address_space(target_cr3, || -> Result<(), &'static str> {
        // Deduplicate mapped code pages because multiple symbols may share a page.
        let mut mapped_pages = Vec::with_capacity(
            required_kernel_function_vas.len() * USER_CURSOR_CODE_PAGES_PER_SYMBOL as usize,
        );

        for fn_va in required_kernel_function_vas {
            let code_kernel_page_va = fn_va & !0xFFFu64;

            for page_idx in 0..USER_CURSOR_CODE_PAGES_PER_SYMBOL {
                let candidate_kernel_page_va =
                    code_kernel_page_va.saturating_add(page_idx * pmm::PAGE_SIZE);

                if mapped_pages.contains(&candidate_kernel_page_va) {
                    continue;
                }

                let code_phys = crate::kernel_va_to_phys(candidate_kernel_page_va)
                    .ok_or("cursor demo entry has non-higher-half address")?;
                let code_user_page_va = crate::kernel_va_to_user_code_va(candidate_kernel_page_va)
                    .ok_or("cursor demo function outside user alias window")?;

                vmm::map_user_page(code_user_page_va, code_phys / pmm::PAGE_SIZE, false)
                    .map_err(|_| "mapping user code page failed")?;

                mapped_pages.push(candidate_kernel_page_va);
            }
        }

        vmm::map_user_page(USER_CURSOR_TASK_STACK_PAGE_VA, stack_frame.pfn, true)
            .map_err(|_| "mapping user stack page failed")?;

        vmm::map_user_page(USER_CURSOR_TASK_DATA_VA, data_frame.pfn, false)
            .map_err(|_| "mapping user data page failed")?;

        Ok(())
    })
}

/// Writes all message bytes into the mapped user data page.
///
/// Data layout is a contiguous sequence defined by `CURSOR_MSG_*_OFFSET`.
/// The page is zeroed first for deterministic content and cleaner debugging.
fn write_cursor_data_page(target_cr3: u64) -> Result<(), &'static str> {
    vmm::with_address_space(target_cr3, || {
        // SAFETY:
        // - This requires `unsafe` because raw memory copy operations require manually proving non-overlap and valid ranges.
        // - `USER_CURSOR_TASK_DATA_VA` is mapped writable in `map_cursor_task_pages`.
        // - All writes are bounded to the single 4 KiB data page.
        unsafe {
            let base = USER_CURSOR_TASK_DATA_VA as *mut u8;
            core::ptr::write_bytes(base, 0, 0x1000);

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_START.as_ptr(),
                base.add(CURSOR_MSG_START_OFFSET),
                CURSOR_MSG_START.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_PASS_ROUNDTRIP.as_ptr(),
                base.add(CURSOR_MSG_PASS_ROUNDTRIP_OFFSET),
                CURSOR_MSG_PASS_ROUNDTRIP.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_FAIL_ROUNDTRIP.as_ptr(),
                base.add(CURSOR_MSG_FAIL_ROUNDTRIP_OFFSET),
                CURSOR_MSG_FAIL_ROUNDTRIP.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_PASS_CLAMP.as_ptr(),
                base.add(CURSOR_MSG_PASS_CLAMP_OFFSET),
                CURSOR_MSG_PASS_CLAMP.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_FAIL_CLAMP.as_ptr(),
                base.add(CURSOR_MSG_FAIL_CLAMP_OFFSET),
                CURSOR_MSG_FAIL_CLAMP.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_PASS_NL.as_ptr(),
                base.add(CURSOR_MSG_PASS_NL_OFFSET),
                CURSOR_MSG_PASS_NL.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_FAIL_NL.as_ptr(),
                base.add(CURSOR_MSG_FAIL_NL_OFFSET),
                CURSOR_MSG_FAIL_NL.len(),
            );
            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_SUMMARY_OK.as_ptr(),
                base.add(CURSOR_MSG_SUMMARY_OK_OFFSET),
                CURSOR_MSG_SUMMARY_OK.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_SUMMARY_FAIL.as_ptr(),
                base.add(CURSOR_MSG_SUMMARY_FAIL_OFFSET),
                CURSOR_MSG_SUMMARY_FAIL.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_ERR.as_ptr(),
                base.add(CURSOR_MSG_ERR_OFFSET),
                CURSOR_MSG_ERR.len(),
            );
            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_WAIT_ESC.as_ptr(),
                base.add(CURSOR_MSG_WAIT_ESC_OFFSET),
                CURSOR_MSG_WAIT_ESC.len(),
            );
            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_EXIT.as_ptr(),
                base.add(CURSOR_MSG_EXIT_OFFSET),
                CURSOR_MSG_EXIT.len(),
            );

            core::ptr::copy_nonoverlapping(
                CURSOR_MSG_NL_ONLY.as_ptr(),
                base.add(CURSOR_MSG_NL_ONLY_OFFSET),
                CURSOR_MSG_NL_ONLY.len(),
            );
        }

        Ok(())
    })
}

/// Ring-3 demo task entry.
///
/// Test sequence:
/// 0. `ClearScreen()` to guarantee deterministic visual output.
/// 1. `SetCursor(3,5)` then `GetCursor()` => expect `(3,5)` (roundtrip).
/// 2. `SetCursor(usize::MAX, usize::MAX)` => expect clamp to `(24,79)`.
/// 3. `SetCursor(10,0)`, print newline, then `GetCursor()` => expect `(11,0)`.
/// 4. Wait for ESC key; on ESC clear screen again and print exit message.
///
/// Output:
/// - emits PASS/FAIL line per check,
/// - emits final summary line,
/// - always exits via `sys_exit`.
extern "C" fn cursor_ring3_task() -> ! {
    let mut failures = 0usize;

    match syscall::user::sys_clear_screen() {
        Ok(()) => {}
        Err(_) => {
            failures += 1;
        }
    }

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `USER_CURSOR_TASK_DATA_VA` points to a mapped read-only user data page.
    // - Offsets/lengths used below are compile-time bounded within that page.
    unsafe {
        let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_START_OFFSET as u64) as *const u8;
        let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_START.len());
    }

    let roundtrip_ok = match syscall::user::sys_set_cursor(3, 5) {
        Ok(()) => match syscall::user::sys_get_cursor() {
            Ok((row, col)) => row == 3 && col == 5,
            Err(_) => false,
        },
        Err(_) => false,
    };

    if roundtrip_ok {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr =
                (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_PASS_ROUNDTRIP_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_PASS_ROUNDTRIP.len());
        }
    } else {
        failures += 1;
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr =
                (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_FAIL_ROUNDTRIP_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_FAIL_ROUNDTRIP.len());
        }
    }

    // Clamping contract check at extreme coordinates.
    let clamp_ok = match syscall::user::sys_set_cursor(usize::MAX, usize::MAX) {
        Ok(()) => match syscall::user::sys_get_cursor() {
            Ok((row, col)) => row == 24 && col == 79,
            Err(_) => false,
        },
        Err(_) => false,
    };

    if clamp_ok {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_PASS_CLAMP_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_PASS_CLAMP.len());
        }
    } else {
        failures += 1;
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_FAIL_CLAMP_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_FAIL_CLAMP.len());
        }
    }

    // Newline transition contract check.
    let newline_ok = match syscall::user::sys_set_cursor(10, 0) {
        Ok(()) => {
            // SAFETY:
            // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
            // - Data page is mapped user-readable; offset/len are in-bounds constants.
            unsafe {
                let ptr =
                    (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_NL_ONLY_OFFSET as u64) as *const u8;
                let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_NL_ONLY.len());
            }
            match syscall::user::sys_get_cursor() {
                Ok((row, col)) => row == 11 && col == 0,
                Err(_) => false,
            }
        }
        Err(_) => false,
    };

    if newline_ok {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_PASS_NL_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_PASS_NL.len());
        }
    } else {
        failures += 1;
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_FAIL_NL_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_FAIL_NL.len());
        }
    }

    // Final status line.
    if failures == 0 {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_SUMMARY_OK_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_SUMMARY_OK.len());
        }
    } else {
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr =
                (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_SUMMARY_FAIL_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_SUMMARY_FAIL.len());
        }
        // SAFETY:
        // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
        // - Data page is mapped user-readable; offset/len are in-bounds constants.
        unsafe {
            let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_ERR_OFFSET as u64) as *const u8;
            let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_ERR.len());
        }
    }

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Data page is mapped user-readable; offset/len are in-bounds constants.
    unsafe {
        let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_WAIT_ESC_OFFSET as u64) as *const u8;
        let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_WAIT_ESC.len());
    }

    loop {
        match syscall::user::sys_getchar() {
            Ok(0x1B) => break,
            Ok(_) => {}
            Err(_) => {}
        }
    }

    let _ = syscall::user::sys_clear_screen();
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Data page is mapped user-readable; offset/len are in-bounds constants.
    unsafe {
        let ptr = (USER_CURSOR_TASK_DATA_VA + CURSOR_MSG_EXIT_OFFSET as u64) as *const u8;
        let _ = syscall::user::sys_write_console(ptr, CURSOR_MSG_EXIT.len());
    }

    syscall::user::sys_exit();
}
