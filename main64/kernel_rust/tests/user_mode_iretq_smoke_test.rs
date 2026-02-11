//! Ring-3 iretq path smoke test.
//!
//! Boots kernel subsystems, builds one dummy user task, and verifies that the
//! scheduler/IRQ `iretq` path reaches ring 3 code by observing a user-written
//! flag byte.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};
use kaos_kernel::arch::{gdt, interrupts};
use kaos_kernel::memory::{pmm, vmm};
use kaos_kernel::scheduler;

const USER_CODE_VA: u64 = vmm::USER_CODE_BASE;
const USER_FLAG_VA: u64 = vmm::USER_CODE_BASE + 0x1000;
const USER_STACK_PAGE_VA: u64 = vmm::USER_STACK_TOP - 0x1000;
const USER_STACK_TOP_ALIGNED: u64 = vmm::USER_STACK_TOP - 16;
const USER_FLAG_VALUE: u8 = 0x2A;
static USER_TASK_OBSERVED: AtomicBool = AtomicBool::new(false);

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();
    pmm::init(false);
    interrupts::init();
    vmm::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

fn write_user_stub(code_va: u64, flag_va: u64, flag_value: u8) {
    // Stub:
    //   mov al, imm8
    //   mov [moffs64], al
    //   jmp $
    //
    // Encoding:
    //   B0 xx
    //   A2 imm64
    //   EB FE
    let mut code = [0u8; 13];
    code[0] = 0xB0;
    code[1] = flag_value;
    code[2] = 0xA2;
    code[3..11].copy_from_slice(&flag_va.to_le_bytes());
    code[11] = 0xEB;
    code[12] = 0xFE;

    // SAFETY:
    // - `code_va` points to a mapped writable user page prepared by test setup.
    // - Copy length is exactly the assembled stub length.
    unsafe {
        core::ptr::copy_nonoverlapping(code.as_ptr(), code_va as *mut u8, code.len());
    }
}

extern "C" fn observer_task() -> ! {
    loop {
        // SAFETY: `USER_FLAG_VA` is mapped before this task is spawned.
        let value = unsafe { core::ptr::read_volatile(USER_FLAG_VA as *const u8) };
        if value == USER_FLAG_VALUE {
            USER_TASK_OBSERVED.store(true, Ordering::Release);
            scheduler::request_stop();
            scheduler::yield_now();
        }
        core::hint::spin_loop();
    }
}

/// Contract: one user task is reached via scheduler irq iretq path.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "one user task is reached via scheduler irq iretq path".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_one_user_task_is_reached_via_scheduler_irq_iretq_path() {
    USER_TASK_OBSERVED.store(false, Ordering::Release);

    // Map user code/flag/stack pages in configured user regions.
    let code_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("alloc code frame failed"));
    let flag_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("alloc flag frame failed"));
    let stack_frame = pmm::with_pmm(|mgr| mgr.alloc_frame().expect("alloc stack frame failed"));

    vmm::map_user_page(USER_CODE_VA, code_frame.pfn, true).expect("map user code page failed");
    vmm::map_user_page(USER_FLAG_VA, flag_frame.pfn, true).expect("map user flag page failed");
    vmm::map_user_page(USER_STACK_PAGE_VA, stack_frame.pfn, true).expect("map user stack page failed");

    write_user_stub(USER_CODE_VA, USER_FLAG_VA, USER_FLAG_VALUE);

    // Ensure flag is initially zero.
    // SAFETY: `USER_FLAG_VA` is mapped writable above.
    unsafe {
        core::ptr::write_volatile(USER_FLAG_VA as *mut u8, 0);
    }

    scheduler::init();
    let cr3 = vmm::get_pml4_address();
    scheduler::set_kernel_address_space_cr3(cr3);
    scheduler::spawn_kernel_task(observer_task).expect("observer kernel task spawn should succeed");
    scheduler::spawn_user_task(USER_CODE_VA, USER_STACK_TOP_ALIGNED, cr3)
        .expect("spawn_user should succeed");
    scheduler::start();

    interrupts::init_periodic_timer(250);
    interrupts::enable();

    let mut observed = false;
    for _ in 0..5_000_000usize {
        if USER_TASK_OBSERVED.load(Ordering::Acquire) && !scheduler::is_running() {
            observed = true;
            break;
        }
        core::hint::spin_loop();
    }

    interrupts::disable();

    assert!(
        observed,
        "user-mode stub did not run; expected flag write via iretq ring3 transition"
    );
}
