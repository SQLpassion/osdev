//! Integration tests for the RTL8139 user-space driver lifecycle and abstractions.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec;
use core::panic::PanicInfo;

use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
use kaos_kernel::scheduler::{
    self as sched, set_running_slot_for_test, set_task_caps, task_id_slot,
};
use kaos_kernel::syscall::{dispatch_checked, SyscallId, SYSCALL_OK};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);
    sched::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

extern "C" fn test_task_loop() -> ! {
    loop {
        sched::yield_now();
    }
}

/// Tests that a simulated RTL8139 driver task with MMIO and IRQ capabilities
/// can map its device registers and subscribe to its interrupt line.
#[test_case]
fn test_rtl8139_driver_grants_and_mapping() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Simulated RTL8139 BAR physical 0xFEB0_0000, 256 bytes, IRQ line 11
    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::MMIO | Capabilities::IRQ,
        grants,
    )));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Map RTL8139 MMIO BAR
    let va = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 256, 0, 0)
        .expect("RTL8139 MMIO mapping must succeed");
    assert_eq!(
        va,
        vmm::USER_MMIO_BASE,
        "MMIO mapping starts at USER_MMIO_BASE"
    );

    // Subscribe to RTL8139 IRQ
    let irq_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        irq_res,
        Ok(SYSCALL_OK),
        "RTL8139 IRQ subscription must succeed"
    );

    // Clean up
    let unmap_res = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va, 256, 0, 0);
    assert_eq!(unmap_res, Ok(SYSCALL_OK), "Unmap MMIO must succeed");

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    kaos_kernel::drivers::irq_bridge::reset_bindings_for_test();
}
