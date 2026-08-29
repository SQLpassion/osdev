//! Integration tests for the SpawnDriver syscall.

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
use kaos_kernel::syscall::{dispatch_checked, SyscallError, SyscallId, UserDriverGrants};

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

/// Tests that SpawnDriver fails with PermissionDenied when calling task lacks SPAWN_DRIVER.
#[test_case]
fn test_spawn_driver_without_capability_fails() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Give task only MMIO capability (no SPAWN_DRIVER)
    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let res = dispatch_checked(SyscallId::SPAWN_DRIVER, 0, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "SpawnDriver without SPAWN_DRIVER capability must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that SpawnDriver fails with InvalidArg on null or kernel-space filename pointer.
#[test_case]
fn test_spawn_driver_invalid_name_pointer() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Give task SPAWN_DRIVER capability
    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::SPAWN_DRIVER,
        grants,
    )));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Null pointer
    let res_null = dispatch_checked(SyscallId::SPAWN_DRIVER, 0, 0, 0, 0);
    assert_eq!(
        res_null,
        Err(SyscallError::InvalidArg),
        "SpawnDriver with null name pointer must return InvalidArg"
    );

    // Kernel-space pointer
    static KERNEL_STR: &[u8] = b"RTL8139.BIN\0";
    let res_kernel = dispatch_checked(SyscallId::SPAWN_DRIVER, KERNEL_STR.as_ptr() as u64, 0, 0, 0);
    assert_eq!(
        res_kernel,
        Err(SyscallError::InvalidArg),
        "SpawnDriver with kernel-space name pointer must return InvalidArg"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that UserDriverGrants layout matches the expected ABI struct size and alignment.
#[test_case]
fn test_user_driver_grants_layout() {
    assert_eq!(
        core::mem::size_of::<UserDriverGrants>(),
        24,
        "UserDriverGrants size must be exactly 24 bytes"
    );
    assert_eq!(
        core::mem::align_of::<UserDriverGrants>(),
        8,
        "UserDriverGrants alignment must be 8 bytes"
    );
}
