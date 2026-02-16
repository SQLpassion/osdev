use crate::drivers::screen::with_screen;
use crate::memory::pmm;
use crate::memory::vmm;
use crate::scheduler;
use crate::syscall;
use core::fmt::Write;

/// User mode echo demo constants.
const USER_ECHO_TASK_STACK_PAGE_VA: u64 = vmm::USER_STACK_TOP - 0x1000;
const USER_ECHO_TASK_STACK_TOP: u64 = vmm::USER_STACK_TOP - 16;
const USER_ECHO_TASK_DATA_VA: u64 = vmm::USER_STACK_TOP - 0x2000;

/// Runs a ring-3 echo demo:
/// - map required code/data/stack pages for the echo task,
/// - spawn a user task that performs GetChar/WriteConsole in a loop,
/// - block until the task exits (on ESC key).
pub(crate) fn run_user_mode_echo_demo() {
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    if let Err(msg) = map_echo_task_pages(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "Echo demo setup failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    if let Err(msg) = write_echo_data_page(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "Echo demo data write failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    // Translate kernel VA of echo_ring3_task to user-code alias window.
    let entry_kernel_va = echo_ring3_task as *const () as usize as u64;
    let entry_rip = match crate::kernel_va_to_user_code_va(entry_kernel_va) {
        Some(va) => va,
        None => {
            with_screen(|screen| {
                writeln!(
                    screen,
                    "Echo demo spawn failed: entry address outside user code alias window"
                )
                .unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    let task_id = match scheduler::spawn_user_task(entry_rip, USER_ECHO_TASK_STACK_TOP, user_cr3) {
        Ok(task_id) => task_id,
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "Echo demo spawn failed: {:?}", err).unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    // Wait until the user task is finished.
    while scheduler::task_frame_ptr(task_id).is_some() {
        scheduler::yield_now();
    }
}

/// Maps all pages needed by `echo_ring3_task`:
/// - code pages for the echo entry, syscall wrappers, and ABI helpers,
/// - one writable stack page.
fn map_echo_task_pages(target_cr3: u64) -> Result<(), &'static str> {
    // Map required kernel functions into user-code alias window.
    // Include syscall0, syscall1, and syscall2 to cover all potential ABI paths
    // that the compiler might generate for error handling and Result unwrapping.
    let required_kernel_function_vas: [u64; 7] = [
        echo_ring3_task as *const () as usize as u64,
        syscall::user::sys_getchar as *const () as usize as u64,
        syscall::user::sys_write_console as *const () as usize as u64,
        syscall::user::sys_exit as *const () as usize as u64,
        syscall::abi::syscall0 as *const () as usize as u64,
        syscall::abi::syscall1 as *const () as usize as u64,
        syscall::abi::syscall2 as *const () as usize as u64,
    ];

    // Allocate stack and data frames.
    let stack_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user echo task stack frame")
    });

    let data_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user echo task data frame")
    });

    vmm::with_address_space(target_cr3, || -> Result<(), &'static str> {
        // Map all code pages, deduplicating pages if multiple functions share one.
        // Track mapped pages to avoid double-mapping.
        let mut mapped_pages = [0u64; 16];
        let mut mapped_count = 0usize;

        for fn_va in required_kernel_function_vas {
            // Page alignment: Round function address down to 4 KiB page boundary.
            // Pages in x86-64 are always 4 KiB (0x1000 bytes) aligned, so we must
            // mask off the lower 12 bits to get the page-aligned address.
            let code_kernel_page_va = fn_va & !0xFFFu64;

            // Skip if this page was already mapped.
            if mapped_pages[..mapped_count].contains(&code_kernel_page_va) {
                continue;
            }

            let code_phys = crate::kernel_va_to_phys(code_kernel_page_va)
                .ok_or("echo demo entry has non-higher-half address")?;
            let code_user_page_va = crate::kernel_va_to_user_code_va(code_kernel_page_va)
                .ok_or("echo demo function outside user alias window")?;
            vmm::map_user_page(code_user_page_va, code_phys / pmm::PAGE_SIZE, false)
                .map_err(|_| "mapping user code page failed")?;

            // Track this page as mapped.
            if mapped_count < mapped_pages.len() {
                mapped_pages[mapped_count] = code_kernel_page_va;
                mapped_count += 1;
            }
        }

        // Map stack page.
        vmm::map_user_page(USER_ECHO_TASK_STACK_PAGE_VA, stack_frame.pfn, true)
            .map_err(|_| "mapping user stack page failed")?;

        // Map data page (for string constants).
        vmm::map_user_page(USER_ECHO_TASK_DATA_VA, data_frame.pfn, false)
            .map_err(|_| "mapping user data page failed")?;

        Ok(())
    })
}

/// Writes string constants into the user data page.
fn write_echo_data_page(target_cr3: u64) -> Result<(), &'static str> {
    const WELCOME_MSG: &[u8] = b"[ring3] Echo task running. Type characters (ESC=exit)\n";
    const ERR_MSG: &[u8] = b"[ring3] GetChar failed!\n";
    const EXIT_MSG: &[u8] = b"\n[ring3] ESC pressed. Exiting echo demo...\n";
    const NL: &[u8] = b"\n";

    vmm::with_address_space(target_cr3, || {
        // SAFETY: USER_ECHO_TASK_DATA_VA is mapped in map_echo_task_pages.
        unsafe {
            let base = USER_ECHO_TASK_DATA_VA as *mut u8;

            // Clear the page first.
            core::ptr::write_bytes(base, 0, 0x1000);

            // Layout: WELCOME_MSG, ERR_MSG, EXIT_MSG, NL.
            let mut offset = 0usize;

            core::ptr::copy_nonoverlapping(
                WELCOME_MSG.as_ptr(),
                base.add(offset),
                WELCOME_MSG.len(),
            );
            offset += WELCOME_MSG.len();

            core::ptr::copy_nonoverlapping(ERR_MSG.as_ptr(), base.add(offset), ERR_MSG.len());
            offset += ERR_MSG.len();

            core::ptr::copy_nonoverlapping(EXIT_MSG.as_ptr(), base.add(offset), EXIT_MSG.len());
            offset += EXIT_MSG.len();

            core::ptr::copy_nonoverlapping(NL.as_ptr(), base.add(offset), NL.len());
        }
        Ok(())
    })
}

/// Ring-3 echo task entry executed via scheduler `iretq`.
///
/// Demonstrates the GetChar syscall by reading characters in a loop
/// and echoing them back via WriteConsole.
extern "C" fn echo_ring3_task() -> ! {
    // String constants are stored in the mapped data page.
    const WELCOME_LEN: usize = 54; // Length of welcome message.
    const ERR_OFFSET: usize = WELCOME_LEN;
    const ERR_LEN: usize = 24;
    const EXIT_OFFSET: usize = ERR_OFFSET + ERR_LEN;
    const EXIT_LEN: usize = 44;
    const NL_OFFSET: usize = EXIT_OFFSET + EXIT_LEN;

    // Print welcome message.
    // SAFETY: USER_ECHO_TASK_DATA_VA points to the initialized readonly data page.
    unsafe {
        let welcome_ptr = USER_ECHO_TASK_DATA_VA as *const u8;
        let _ = syscall::user::sys_write_console(welcome_ptr, WELCOME_LEN);
    }

    loop {
        // Block until a character is available (via GetChar syscall).
        let ch = match syscall::user::sys_getchar() {
            Ok(ch) => ch,
            Err(_) => {
                // SAFETY: ERR segment is inside the initialized data page.
                unsafe {
                    let err_ptr = (USER_ECHO_TASK_DATA_VA + ERR_OFFSET as u64) as *const u8;
                    let _ = syscall::user::sys_write_console(err_ptr, ERR_LEN);
                }

                syscall::user::sys_exit();
            }
        };

        // ESC key (0x1B) - exit.
        if ch == 0x1B {
            // SAFETY: EXIT segment is inside the initialized data page.
            unsafe {
                let exit_ptr = (USER_ECHO_TASK_DATA_VA + EXIT_OFFSET as u64) as *const u8;
                let _ = syscall::user::sys_write_console(exit_ptr, EXIT_LEN);
            }

            syscall::user::sys_exit();
        }

        // Echo the character back.
        // SAFETY: `&ch` is valid for one byte for the duration of the syscall.
        unsafe {
            let _ = syscall::user::sys_write_console(&ch as *const u8, 1);
        }

        // Handle newline.
        if ch == b'\n' || ch == b'\r' {
            // SAFETY: NL byte is inside the initialized data page.
            unsafe {
                let nl_ptr = (USER_ECHO_TASK_DATA_VA + NL_OFFSET as u64) as *const u8;
                let _ = syscall::user::sys_write_console(nl_ptr, 1);
            }
        }
    }
}
