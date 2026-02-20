use crate::drivers::screen::with_screen;
use crate::memory::pmm;
use crate::memory::vmm;
use crate::scheduler;
use crate::syscall;
use core::fmt::Write;

/// User mode readline demo constants.
const USER_READLINE_TASK_STACK_PAGE_VA: u64 = vmm::USER_STACK_TOP - 0x1000;
const USER_READLINE_TASK_STACK_TOP: u64 = vmm::USER_STACK_TOP - 16;
const USER_READLINE_TASK_DATA_VA: u64 = vmm::USER_STACK_TOP - 0x2000;
const USER_READLINE_TASK_LINE_BUF_LEN: usize = 96;
const USER_READLINE_CODE_PAGES_PER_SYMBOL: u64 = 2;
const USER_READLINE_PROMPT: &[u8] = b"Enter your name: ";
const USER_READLINE_ECHO_PREFIX: &[u8] = b"Your name is: ";
const USER_READLINE_ERR: &[u8] = b"[ring3] user_readline failed\n";
const USER_READLINE_NL: &[u8] = b"\n";
const USER_READLINE_PROMPT_OFFSET: usize = 0;
const USER_READLINE_ECHO_PREFIX_OFFSET: usize =
    USER_READLINE_PROMPT_OFFSET + USER_READLINE_PROMPT.len();
const USER_READLINE_ERR_OFFSET: usize =
    USER_READLINE_ECHO_PREFIX_OFFSET + USER_READLINE_ECHO_PREFIX.len();
const USER_READLINE_NL_OFFSET: usize = USER_READLINE_ERR_OFFSET + USER_READLINE_ERR.len();

/// Runs a ring-3 readline demo:
/// - map required code/data/stack pages for the readline task,
/// - spawn a user task that calls `user_readline`,
/// - block until the task exits.
pub(crate) fn run_user_mode_readline_demo() {
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    if let Err(msg) = map_readline_task_pages(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "Readline demo setup failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    if let Err(msg) = write_readline_data_page(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "Readline demo data write failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    let entry_kernel_va = readline_ring3_task as *const () as usize as u64;
    let entry_rip = match crate::kernel_va_to_user_code_va(entry_kernel_va) {
        Some(va) => va,
        None => {
            with_screen(|screen| {
                writeln!(
                    screen,
                    "Readline demo spawn failed: entry address outside user code alias window"
                )
                .unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    let task_id =
        match scheduler::spawn_user_task(entry_rip, USER_READLINE_TASK_STACK_TOP, user_cr3) {
            Ok(task_id) => task_id,
            Err(err) => {
                with_screen(|screen| {
                    writeln!(screen, "Readline demo spawn failed: {:?}", err).unwrap();
                });
                vmm::destroy_user_address_space(user_cr3);
                return;
            }
        };

    while scheduler::task_frame_ptr(task_id).is_some() {
        scheduler::yield_now();
    }
}

/// Maps all pages needed by `readline_ring3_task`:
/// - code pages for readline entry and required syscall wrappers/helpers,
/// - one writable stack page,
/// - one user-readable data page for prompt/status strings.
fn map_readline_task_pages(target_cr3: u64) -> Result<(), &'static str> {
    let required_kernel_function_vas: [u64; 8] = [
        readline_ring3_task as *const () as usize as u64,
        syscall::user::user_readline as *const () as usize as u64,
        syscall::user::sys_getchar as *const () as usize as u64,
        syscall::user::sys_write_console as *const () as usize as u64,
        syscall::user::sys_exit as *const () as usize as u64,
        syscall::abi::syscall0 as *const () as usize as u64,
        syscall::abi::syscall1 as *const () as usize as u64,
        syscall::abi::syscall2 as *const () as usize as u64,
    ];

    let stack_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user readline task stack frame")
    });

    let data_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user readline task data frame")
    });

    vmm::with_address_space(target_cr3, || -> Result<(), &'static str> {
        let mut mapped_pages = [0u64; 16];
        let mut mapped_count = 0usize;

        for fn_va in required_kernel_function_vas {
            // Page alignment: Round function address down to 4 KiB page boundary.
            // Pages in x86-64 are always 4 KiB (0x1000 bytes) aligned, so we must
            // mask off the lower 12 bits to get the page-aligned address.
            let code_kernel_page_va = fn_va & !0xFFFu64;

            // Some monomorphized Rust functions can cross a 4 KiB boundary in
            // debug builds; map the first N contiguous pages per symbol.
            for page_idx in 0..USER_READLINE_CODE_PAGES_PER_SYMBOL {
                let candidate_kernel_page_va =
                    code_kernel_page_va.saturating_add(page_idx * pmm::PAGE_SIZE);

                if mapped_pages[..mapped_count].contains(&candidate_kernel_page_va) {
                    continue;
                }

                let code_phys = crate::kernel_va_to_phys(candidate_kernel_page_va)
                    .ok_or("readline demo entry has non-higher-half address")?;

                let code_user_page_va = crate::kernel_va_to_user_code_va(candidate_kernel_page_va)
                    .ok_or("readline demo function outside user alias window")?;

                vmm::map_user_page(code_user_page_va, code_phys / pmm::PAGE_SIZE, false)
                    .map_err(|_| "mapping user code page failed")?;

                if mapped_count < mapped_pages.len() {
                    mapped_pages[mapped_count] = candidate_kernel_page_va;
                    mapped_count += 1;
                }
            }
        }

        vmm::map_user_page(USER_READLINE_TASK_STACK_PAGE_VA, stack_frame.pfn, true)
            .map_err(|_| "mapping user stack page failed")?;
        vmm::map_user_page(USER_READLINE_TASK_DATA_VA, data_frame.pfn, false)
            .map_err(|_| "mapping user data page failed")?;

        Ok(())
    })
}

/// Writes readonly message bytes for the ring-3 readline task.
fn write_readline_data_page(target_cr3: u64) -> Result<(), &'static str> {
    vmm::with_address_space(target_cr3, || {
        // SAFETY: USER_READLINE_TASK_DATA_VA is mapped in map_readline_task_pages.
        // - This requires `unsafe` because raw memory copy operations require manually proving non-overlap and valid ranges.
        unsafe {
            let base = USER_READLINE_TASK_DATA_VA as *mut u8;
            core::ptr::write_bytes(base, 0, 0x1000);

            core::ptr::copy_nonoverlapping(
                USER_READLINE_PROMPT.as_ptr(),
                base.add(USER_READLINE_PROMPT_OFFSET),
                USER_READLINE_PROMPT.len(),
            );
            core::ptr::copy_nonoverlapping(
                USER_READLINE_ECHO_PREFIX.as_ptr(),
                base.add(USER_READLINE_ECHO_PREFIX_OFFSET),
                USER_READLINE_ECHO_PREFIX.len(),
            );
            core::ptr::copy_nonoverlapping(
                USER_READLINE_ERR.as_ptr(),
                base.add(USER_READLINE_ERR_OFFSET),
                USER_READLINE_ERR.len(),
            );
            core::ptr::copy_nonoverlapping(
                USER_READLINE_NL.as_ptr(),
                base.add(USER_READLINE_NL_OFFSET),
                USER_READLINE_NL.len(),
            );
        }
        Ok(())
    })
}

/// Ring-3 readline task entry executed via scheduler `iretq`.
///
/// Demonstrates user-space line editing by calling `user_readline`.
extern "C" fn readline_ring3_task() -> ! {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - USER_READLINE_TASK_DATA_VA is mapped as user-readable data page.
    // - PROMPT offset/length are within the single initialized 4 KiB data page.
    unsafe {
        let prompt_ptr =
            (USER_READLINE_TASK_DATA_VA + USER_READLINE_PROMPT_OFFSET as u64) as *const u8;
        let _ = syscall::user::sys_write_console(prompt_ptr, USER_READLINE_PROMPT.len());
    }

    let mut line = [0u8; USER_READLINE_TASK_LINE_BUF_LEN];
    let line_len = match syscall::user::user_readline(&mut line) {
        Ok(len) => len,
        Err(_) => {
            // SAFETY:
            // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
            // - USER_READLINE_TASK_DATA_VA is mapped as user-readable data page.
            // - ERR offset/length are within the initialized data page.
            unsafe {
                let err_ptr =
                    (USER_READLINE_TASK_DATA_VA + USER_READLINE_ERR_OFFSET as u64) as *const u8;
                let _ = syscall::user::sys_write_console(err_ptr, USER_READLINE_ERR.len());
            }

            syscall::user::sys_exit();
        }
    };

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - USER_READLINE_TASK_DATA_VA is mapped as user-readable data page.
    // - Prefix/NL offsets and lengths are bounded to the initialized data page.
    // - `line.as_ptr()` is valid for `line_len` bytes by construction (`line_len <= line.len()`).
    unsafe {
        let prefix_ptr =
            (USER_READLINE_TASK_DATA_VA + USER_READLINE_ECHO_PREFIX_OFFSET as u64) as *const u8;
        let _ = syscall::user::sys_write_console(prefix_ptr, USER_READLINE_ECHO_PREFIX.len());

        if line_len > 0 {
            let _ = syscall::user::sys_write_console(line.as_ptr(), line_len);
        }

        let nl_ptr = (USER_READLINE_TASK_DATA_VA + USER_READLINE_NL_OFFSET as u64) as *const u8;
        let _ = syscall::user::sys_write_console(nl_ptr, USER_READLINE_NL.len());
    }

    syscall::user::sys_exit();
}
