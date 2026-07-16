//! Ring-3 invalid-opcode recovery integration test.
//!
//! Loads the interactive `EXCEPT.BIN` user program, supplies its `U` selection,
//! and verifies that its real `ud2` instruction terminates only that task while
//! a kernel observer task continues running.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use kaos_kernel::arch::{gdt, interrupts};
use kaos_kernel::drivers::{ata, block, keyboard};
use kaos_kernel::io::{fat32, vfs};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process;
use kaos_kernel::scheduler::{self, TaskState};

const NO_TASK: usize = usize::MAX;
const VGA_TEXT_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;
const VGA_ROWS: usize = 25;
const USER_UD_MESSAGE_PREFIX: &[u8] = b"USER EXCEPTION #UD: terminating task at rip=0x";

static FAULTING_TASK_ID: AtomicUsize = AtomicUsize::new(NO_TASK);
static KERNEL_SURVIVED_USER_UD: AtomicBool = AtomicBool::new(false);

/// Returns whether a VGA text row begins with `prefix`.
fn vga_contains_row_prefix(prefix: &[u8]) -> bool {
    // Step 1: Reject a prefix that cannot fit in a single VGA text row.
    if prefix.len() > VGA_COLS {
        return false;
    }

    // Step 2: Compare the requested text against each row start. The diagnostic
    // always starts at column zero because the user program's preceding println
    // has already completed its newline.
    for row in 0..VGA_ROWS {
        let mut matches = true;

        // Step 3: Read actual character cells, ignoring their VGA attribute bytes.
        for (col, expected) in prefix.iter().enumerate() {
            let cell = VGA_TEXT_BUFFER + ((row * VGA_COLS + col) * 2);
            let actual = unsafe {
                // SAFETY:
                // - The test boot path uses the default VGA text console.
                // - The kernel maps the VGA text page at `VGA_TEXT_BUFFER`.
                // - Reading a single character byte from an in-bounds cell is valid.
                // - Volatile access observes the actual MMIO-backed screen content.
                core::ptr::read_volatile(cell as *const u8)
            };

            if actual != *expected {
                matches = false;
                break;
            }
        }

        // Step 4: Stop as soon as one complete matching diagnostic row is found.
        if matches {
            return true;
        }
    }

    false
}

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);
    ata::init();
    block::init_ata();

    // The test runner includes EXCEPT.BIN in its FAT32 image. Mount it through
    // the same VFS path that the interactive shell uses for `exec except.bin`.
    let volume = fat32::Fat32Volume::mount(0).expect("FAT32 test volume must mount");
    vfs::mount(alloc::boxed::Box::new(fat32::Fat32Fs::new(volume)));

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Runs after the user fault and proves that scheduler/kernel execution continues.
extern "C" fn survivor_task() -> ! {
    loop {
        let task_id = FAULTING_TASK_ID.load(Ordering::Acquire);

        // The #UD handler first marks the user task Zombie. A later scheduler
        // pass may already have reaped it by the time this observer runs.
        if task_id != NO_TASK
            && matches!(
                scheduler::task_state(task_id),
                Some(TaskState::Zombie) | None
            )
        {
            KERNEL_SURVIVED_USER_UD.store(true, Ordering::Release);

            // Return control to the test's bootstrap frame after recording the
            // result. The software yield performs the actual frame switch.
            scheduler::request_stop();
            scheduler::yield_now();
        }

        // Give the user task its first time slice while it is still Ready.
        scheduler::yield_now();
    }
}

/// Contract: EXCEPT.BIN's real Ring-3 `ud2` terminates only its task.
/// Given: The mounted EXCEPT.BIN program, an injected `U` menu key, and a scheduler with one kernel observer.
/// When: EXCEPT.BIN displays its menu, reads the key, and raises invalid-opcode exception vector 6.
/// Then: The user task becomes Zombie/reaped, its menu input is discarded, the kernel emits a visible diagnostic, the kernel observer runs, and control returns to the test kernel.
/// Failure Impact: The interactive diagnostic program is missing/broken, the kernel halts, or the faulting user task resumes.
#[test_case]
fn test_ring3_ud2_terminates_only_faulting_task() {
    FAULTING_TASK_ID.store(NO_TASK, Ordering::Release);
    KERNEL_SURVIVED_USER_UD.store(false, Ordering::Release);

    // Step 1: Put the `U` selection into the decoded-key queue before the user
    // program starts. Scancode 0x16 is `u` in the kernel's QWERTZ map.
    keyboard::init();
    keyboard::enqueue_raw_scancode(0x16);
    assert!(
        keyboard::process_pending_scancodes(),
        "injected U scancode must reach the exception exerciser"
    );

    // Step 2: Start the observer first so it can yield into EXCEPT.BIN and
    // later prove that the #UD return path selected another runnable frame.
    scheduler::init();
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());
    scheduler::spawn_kernel_task(survivor_task).expect("spawn survivor task failed");
    let user_task = process::exec_from_vfs("except.bin")
        .expect("EXCEPT.BIN must load and spawn through the VFS process path");
    FAULTING_TASK_ID.store(user_task, Ordering::Release);
    scheduler::start();

    // Step 3: Enable preemption and wait on the bootstrap context until the
    // survivor requests a controlled scheduler stop.
    interrupts::init_periodic_timer(250);
    interrupts::enable();

    let mut recovered = false;
    for _ in 0..5_000_000usize {
        if KERNEL_SURVIVED_USER_UD.load(Ordering::Acquire) && !scheduler::is_running() {
            recovered = true;
            break;
        }
        core::hint::spin_loop();
    }

    interrupts::disable();

    assert!(
        recovered,
        "kernel did not resume a survivor task after Ring-3 #UD"
    );
    assert!(
        scheduler::task_state(user_task).is_none(),
        "faulting Ring-3 task must be reaped after #UD recovery"
    );
    assert!(
        keyboard::read_char().is_none(),
        "Ring-3 #UD recovery must discard the menu key left in the character buffer"
    );
    assert!(
        keyboard::read_key().is_none(),
        "Ring-3 #UD recovery must leave no stale key events for the next task"
    );
    assert!(
        vga_contains_row_prefix(USER_UD_MESSAGE_PREFIX),
        "Ring-3 #UD recovery must show the serial diagnostic prefix on the kernel console"
    );
}
