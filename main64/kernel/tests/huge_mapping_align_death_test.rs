//! Death test for `PageTableEntry::set_huge_mapping`'s alignment contract.
//!
//! `set_huge_mapping` must panic loudly on a frame that is not 2 MiB aligned, rather
//! than silently writing a huge-page leaf whose low bits are misinterpreted as part of
//! the frame address by the CPU. See `kernel/src/memory/vmm/page_table.rs`.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use core::ptr::addr_of_mut;

use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::memory::vmm::page_table::{PageTable, HUGE_PAGE_SIZE_2M};

static mut PD: PageTable = PageTable::new();

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    test_main();

    // The test must panic before reaching this point.
    exit_qemu(QemuExitCode::Failed);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let expected = "is not 2 MiB aligned";
    if kaos_kernel::testing::panic_message_contains(info, expected) {
        exit_qemu(QemuExitCode::Success);
    } else {
        exit_qemu(QemuExitCode::Failed);
    }
}

/// Contract: a misaligned frame passed to `set_huge_mapping` panics with a message
/// naming the alignment violation, instead of writing a corrupt entry.
/// Failure Impact: a silently-accepted misaligned huge-page frame would corrupt address
/// translation the first time the CPU walks that entry — release-blocking.
#[test_case]
fn test_set_huge_mapping_panics_on_misaligned_frame() {
    // SAFETY: single-threaded test context, no concurrent access to PD.
    unsafe {
        (*addr_of_mut!(PD)).zero();
        let entry = &mut (*addr_of_mut!(PD)).entries[0];
        // Not a multiple of HUGE_PAGE_SIZE_2M (0x1000 short).
        entry.set_huge_mapping(HUGE_PAGE_SIZE_2M + 0x1000, true, true, false);
    }
}
