use crate::drivers::screen::with_screen;
use crate::memory::pmm;
use crate::memory::vmm;
use crate::scheduler;
use crate::syscall;
use core::fmt::Write;

/// One mapped user page used as initial ring-3 stack backing store.
const USER_SERIAL_TASK_STACK_PAGE_VA: u64 = vmm::USER_STACK_TOP - 0x1000;
/// Initial user RSP used when creating the demo task frame.
const USER_SERIAL_TASK_STACK_TOP: u64 = vmm::USER_STACK_TOP - 16;
/// User-mapped page that stores the userdemo message bytes.
const USER_SERIAL_TASK_MSG_VA: u64 = vmm::USER_STACK_TOP - 0x2000;
const USER_SERIAL_TASK_MSG: &[u8] = b"[ring3] hello from user mode via int 0x80\n";
const USER_SERIAL_TASK_MSG_LEN: usize = USER_SERIAL_TASK_MSG.len();

/// Runs a minimal ring-3 smoke test:
/// - map required code/data/stack pages for the demo task,
/// - spawn a user task that performs `WriteConsole` then `Exit`,
/// - block until the task has been removed by the scheduler.
pub(crate) fn run_user_mode_serial_demo() {
    let user_cr3 = vmm::clone_kernel_pml4_for_user();

    if let Err(msg) = map_userdemo_task_pages(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "User demo setup failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    if let Err(msg) = write_userdemo_message_page(user_cr3) {
        with_screen(|screen| {
            writeln!(screen, "User demo image failed: {}", msg).unwrap();
        });
        vmm::destroy_user_address_space(user_cr3);
        return;
    }

    // TEMPORARY BOOTSTRAP:
    // `userdemo_ring3_task` is compiled as part of kernel text, not loaded from
    // a user binary. Therefore we derive its kernel VA and translate it into the
    // user-code alias window for ring-3 RIP.
    //
    // Once a real program loader exists, RIP must come from the loaded user
    // executable entry point and this translation path should be removed.
    let entry_kernel_va = userdemo_ring3_task as *const () as usize as u64;
    let entry_rip = match crate::kernel_va_to_user_code_va(entry_kernel_va) {
        Some(va) => va,
        None => {
            with_screen(|screen| {
                writeln!(
                    screen,
                    "User demo spawn failed: entry address outside user code alias window"
                )
                .unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    let task_id = match scheduler::spawn_user_task(entry_rip, USER_SERIAL_TASK_STACK_TOP, user_cr3)
    {
        Ok(task_id) => task_id,
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "User demo spawn failed: {:?}", err).unwrap();
            });
            vmm::destroy_user_address_space(user_cr3);
            return;
        }
    };

    // Wait until the user task is finished, and voluntarily yield the CPU of the REPL task
    while scheduler::task_frame_ptr(task_id).is_some() {
        scheduler::yield_now();
    }
}

/// Ensures every page needed by `userdemo_ring3_task` exists in user space.
///
/// This includes:
/// - code pages for the demo entry, user syscall wrappers, ABI syscall helpers,
///   and wrapper-side helper routines,
/// - one user-readable message page,
/// - one writable stack page.
///
/// TEMPORARY BOOTSTRAP NOTE:
/// The mapped code pages originate from kernel text and are only aliased into
/// user VA so this demo can execute ring-3 before a full program loader exists.
/// In the final architecture, user code pages are provided by the loader
/// (e.g. ELF segments), not by kernel-text alias mappings.
fn map_userdemo_task_pages(target_cr3: u64) -> Result<(), &'static str> {
    // TEMPORARY BOOTSTRAP:
    // Kernel virtual addresses derived from function pointers (`fn as *const ()`).
    // For each entry we map the containing 4 KiB code page into the user-code
    // alias window so ring-3 can execute the full call chain without fetching
    // instructions from supervisor-only pages.
    //
    // Call chain:
    // userdemo_ring3_task -> user::sys_write_serial/user::sys_write_console/user::sys_exit
    // -> abi::syscall2/syscall1 -> int 0x80.
    //
    // Note:
    // User wrappers include local result decoding logic and therefore must have
    // their own code pages executable from the user alias window.
    let required_kernel_function_vas: [u64; 6] = [
        userdemo_ring3_task as *const () as usize as u64,
        syscall::user::sys_write_serial as *const () as usize as u64,
        syscall::user::sys_write_console as *const () as usize as u64,
        syscall::user::sys_exit as *const () as usize as u64,
        syscall::abi::syscall2 as *const () as usize as u64,
        syscall::abi::syscall1 as *const () as usize as u64,
    ];

    // Reserve two physical 4 KiB frames for userdemo private data pages:
    // - `stack_frame`: backing page for initial ring-3 user stack.
    // - `msg_frame`: backing page that stores USER_SERIAL_TASK_MSG bytes.
    //
    // These are only frame allocations here; actual VA placement and user/rw
    // permissions are applied later via `vmm::map_user_page`.
    let stack_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user serial task stack frame")
    });

    let msg_frame = pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("failed to allocate user serial task message frame")
    });

    // For every required function address:
    // 1) page-align kernel VA to the containing 4 KiB code page,
    // 2) translate that kernel page VA to physical address,
    // 3) compute the corresponding user-code alias VA,
    // 4) map user alias VA -> same physical code frame (read-only from user side).
    //
    // This gives ring-3 an executable view of the exact kernel text pages needed
    // by the demo syscall call chain, while keeping data pages separate.
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

            // Skip if this page was already mapped
            if mapped_pages[..mapped_count].contains(&code_kernel_page_va) {
                continue;
            }

            let code_phys = crate::kernel_va_to_phys(code_kernel_page_va)
                .ok_or("user demo entry has non-higher-half address")?;

            let code_user_page_va = crate::kernel_va_to_user_code_va(code_kernel_page_va)
                .ok_or("user demo function outside user alias window")?;

            vmm::map_user_page(code_user_page_va, code_phys / pmm::PAGE_SIZE, false)
                .map_err(|_| "mapping user code page failed")?;

            // Track this page as mapped
            if mapped_count < mapped_pages.len() {
                mapped_pages[mapped_count] = code_kernel_page_va;
                mapped_count += 1;
            }
        }

        vmm::map_user_page(USER_SERIAL_TASK_MSG_VA, msg_frame.pfn, true)
            .map_err(|_| "mapping user message page failed")?;
        vmm::map_user_page(USER_SERIAL_TASK_STACK_PAGE_VA, stack_frame.pfn, true)
            .map_err(|_| "mapping user stack page failed")?;

        Ok(())
    })
}

/// Writes the static userdemo payload into the user message page.
///
/// The page is pre-zeroed so repeated demo runs always observe deterministic
/// memory content, independent of previous payload lengths.
fn write_userdemo_message_page(target_cr3: u64) -> Result<(), &'static str> {
    vmm::with_address_space(target_cr3, || {
        // SAFETY:
        // - `USER_SERIAL_TASK_MSG_VA` is mapped in `map_userdemo_task_pages`.
        // - Message length fits into one mapped 4 KiB message page.
        unsafe {
            core::ptr::write_bytes(USER_SERIAL_TASK_MSG_VA as *mut u8, 0, 0x1000);
            core::ptr::copy_nonoverlapping(
                USER_SERIAL_TASK_MSG.as_ptr(),
                USER_SERIAL_TASK_MSG_VA as *mut u8,
                USER_SERIAL_TASK_MSG_LEN,
            );
        }
        Ok(())
    })
}

/// Ring-3 demo entry executed via scheduler `iretq`.
///
/// The body intentionally performs only two user-wrapper calls:
/// 1. `WriteSerial(msg_ptr, msg_len)`
/// 2. `WriteConsole(msg_ptr, msg_len)`
/// 3. `Exit(0)`
extern "C" fn userdemo_ring3_task() -> ! {
    // SAFETY:
    // - `USER_SERIAL_TASK_MSG_VA` points to a mapped user-readable buffer.
    // - `USER_SERIAL_TASK_MSG_LEN` bytes were copied into that page.
    unsafe {
        let _ = syscall::user::sys_write_serial(
            USER_SERIAL_TASK_MSG_VA as *const u8,
            USER_SERIAL_TASK_MSG_LEN,
        );
        let _ = syscall::user::sys_write_console(
            USER_SERIAL_TASK_MSG_VA as *const u8,
            USER_SERIAL_TASK_MSG_LEN,
        );
        syscall::user::sys_exit();
    }
}
