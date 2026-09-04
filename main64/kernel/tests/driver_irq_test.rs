//! Integration tests for IRQ bridge syscalls (IrqSubscribe / IrqWait / IrqAck).

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec;
use core::panic::PanicInfo;

use kaos_kernel::arch::interrupts::{self, SavedRegisters};
use kaos_kernel::drivers::irq_bridge::{self, driver_irq_trampoline};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
use kaos_kernel::scheduler::{
    self as sched, set_running_slot_for_test, set_task_caps, task_id_slot,
};
use kaos_kernel::syscall::{dispatch_checked, SyscallError, SyscallId, SYSCALL_OK};

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

/// Tests that IrqSubscribe fails with PermissionDenied if the task has no capabilities.
#[test_case]
fn test_irq_subscribe_no_caps_fails() {
    irq_bridge::reset_bindings_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    set_running_slot_for_test(Some(task_id_slot(task_id)));

    let res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "IrqSubscribe without DriverCaps must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that IrqSubscribe fails if the vector is not granted in ResourceGrants.
#[test_case]
fn test_irq_subscribe_unauthorized_vector_fails() {
    irq_bridge::reset_bindings_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Grant vector 10 only
    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![10],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Request vector 11 (unauthorized)
    let res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "IrqSubscribe for ungranted vector must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests subscription, mutual exclusion on subscription, trampoline trigger, wait, and ack.
#[test_case]
fn test_irq_subscribe_trampoline_wait_and_ack() {
    irq_bridge::reset_bindings_for_test();
    let task1_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task 1");
    let task2_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task 2");
    let slot1 = task_id_slot(task1_id);
    let slot2 = task_id_slot(task2_id);

    let grants1 = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps1_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants1)));
    set_task_caps(task1_id, caps1_ptr);

    let grants2 = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps2_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants2)));
    set_task_caps(task2_id, caps2_ptr);

    // Step 1: Task 1 subscribes to IRQ 11
    set_running_slot_for_test(Some(slot1));
    let sub_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        sub_res,
        Ok(SYSCALL_OK),
        "Task 1 should successfully subscribe to IRQ 11"
    );

    // Step 2: Task 2 attempts to subscribe to IRQ 11 -> must fail with InvalidArg (already owned)
    set_running_slot_for_test(Some(slot2));
    let sub2_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        sub2_res,
        Err(SyscallError::InvalidArg),
        "Task 2 subscribing to already-claimed vector must fail"
    );

    // Step 3: Trigger IRQ 11 via trampoline
    let mut regs = SavedRegisters::default();
    driver_irq_trampoline(11, &mut regs);

    // Step 4: Task 1 calls IrqWait -> since pending was set by trampoline, it returns immediately
    set_running_slot_for_test(Some(slot1));
    let wait_res = dispatch_checked(SyscallId::IRQ_WAIT, 11, 0, 0, 0);
    assert_eq!(
        wait_res,
        Ok(SYSCALL_OK),
        "Task 1 should receive pending IRQ 11"
    );

    // Step 5: Task 1 acknowledges the IRQ
    let ack_res = dispatch_checked(SyscallId::IRQ_ACK, 11, 0, 0, 0);
    assert_eq!(ack_res, Ok(SYSCALL_OK), "Task 1 should acknowledge IRQ 11");

    // Step 6: Task 2 calls IrqAck on IRQ 11 -> must fail (not owner)
    set_running_slot_for_test(Some(slot2));
    let ack2_res = dispatch_checked(SyscallId::IRQ_ACK, 11, 0, 0, 0);
    assert_eq!(
        ack2_res,
        Err(SyscallError::InvalidArg),
        "Non-owner Task 2 calling IrqAck must fail"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task1_id);
    sched::terminate_task(task2_id);
    irq_bridge::reset_bindings_for_test();
}

/// Tests that terminating a task (e.g. a crashed driver) releases its IRQ
/// binding, instead of leaving the vector permanently owned by the dead task.
#[test_case]
fn test_irq_binding_released_on_task_terminate() {
    irq_bridge::reset_bindings_for_test();
    let task1_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task 1");
    let slot1 = task_id_slot(task1_id);

    let grants1 = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![12],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps1_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants1)));
    set_task_caps(task1_id, caps1_ptr);

    // Step 1: Task 1 subscribes to IRQ 12 but never calls IrqAck — simulating
    // a driver that crashes (or exits) while still owning the binding.
    set_running_slot_for_test(Some(slot1));
    let sub_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 12, 0, 0, 0);
    assert_eq!(
        sub_res,
        Ok(SYSCALL_OK),
        "Task 1 should successfully subscribe to IRQ 12"
    );
    assert!(
        irq_bridge::is_driver_irq(12),
        "IRQ 12 should be reported as driver-owned after subscribe"
    );

    // Step 2: Terminate task 1 without ever acknowledging the IRQ.
    set_running_slot_for_test(None);
    sched::terminate_task(task1_id);

    assert!(
        !irq_bridge::is_driver_irq(12),
        "IRQ 12 binding must be released once the owning task is terminated"
    );

    // Step 3: A second task requesting the same vector must now succeed —
    // before the fix, `subscribe`'s compare_exchange(0, ...) would keep
    // failing forever because the dead task's ID was never cleared.
    let task2_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task 2");
    let slot2 = task_id_slot(task2_id);

    let grants2 = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![12],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps2_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants2)));
    set_task_caps(task2_id, caps2_ptr);
    set_running_slot_for_test(Some(slot2));

    let sub2_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 12, 0, 0, 0);
    assert_eq!(
        sub2_res,
        Ok(SYSCALL_OK),
        "Task 2 must be able to claim IRQ 12 after task 1's binding was released"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task2_id);
    irq_bridge::reset_bindings_for_test();
}
