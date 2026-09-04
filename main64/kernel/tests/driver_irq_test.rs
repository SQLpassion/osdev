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

/// Tests the IRQ-ack watchdog (`check_stale_bindings`) that bounds how long a
/// driver-subscribed line may sit in-service without an `IrqAck` — see the
/// doc comment on `IrqBinding::isr_set_since_tsc`.
///
/// Only the branches reachable without a genuine hardware INTA cycle are
/// exercised here: a *synthetic* `isr_set_since_tsc` timestamp (set directly
/// via the test-only `set_isr_set_since_for_test`, mirroring how the sibling
/// `test_irq_subscribe_trampoline_wait_and_ack` test calls
/// `driver_irq_trampoline` directly to simulate an IRQ firing) never makes
/// the PIC's real ISR bit for this line actually set, so `is_in_service`
/// stays false throughout. That is exactly why `check_stale_bindings` checks
/// `is_in_service` before forcing an EOI instead of trusting the timestamp
/// alone: a stale-but-not-really-in-service line must just have its
/// bookkeeping cleared, never a forced (spurious) EOI. The "stale AND
/// genuinely in-service" force-EOI branch requires a real hardware
/// interrupt, the same constraint documented on
/// `test_genuine_hardware_irq0_tick_still_sends_eoi` in
/// `interrupts_layout_test.rs`.
#[test_case]
fn test_check_stale_bindings_clears_synthetic_timestamps_without_forcing_eoi() {
    irq_bridge::reset_bindings_for_test();
    interrupts::pic::reset_eoi_count_for_test();

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(task_id_slot(task_id)));
    dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0).expect("subscribe IRQ 11");
    set_running_slot_for_test(None);

    // A fresh timestamp must never be treated as stale.
    irq_bridge::set_isr_set_since_for_test(11, kaos_kernel::drivers::time::rdtsc());
    irq_bridge::check_stale_bindings();
    assert_ne!(
        irq_bridge::isr_set_since_for_test(11),
        Some(0),
        "a fresh timestamp must not be cleared by check_stale_bindings"
    );
    assert_eq!(
        interrupts::pic::eoi_count_for_test(),
        0,
        "a fresh (non-stale) binding must never trigger a forced EOI"
    );

    // `check_stale_bindings` compares an *absolute* rdtsc delta against its
    // fixed watchdog timeout, so a synthetic "since=1" (approximating
    // power-on) only looks sufficiently stale once the CPU has genuinely
    // been running long enough in real time — otherwise `now` itself is
    // still smaller than the timeout. Busy-wait past that real-time floor
    // before relying on it, well past any plausible watchdog timeout.
    let ticks_per_us = kaos_kernel::drivers::time::tsc_ticks_per_us();
    let real_time_floor =
        kaos_kernel::drivers::time::rdtsc().saturating_add(ticks_per_us.saturating_mul(700_000));
    while kaos_kernel::drivers::time::rdtsc() < real_time_floor {
        core::hint::spin_loop();
    }

    // An old timestamp on a line whose PIC ISR bit was never actually
    // latched (no genuine hardware INTA occurred here) must just clear the
    // bookkeeping, never ring a forced EOI it has no real PIC state to
    // justify.
    irq_bridge::set_isr_set_since_for_test(11, 1);
    irq_bridge::check_stale_bindings();
    assert_eq!(
        irq_bridge::isr_set_since_for_test(11),
        Some(0),
        "a stale-but-not-actually-in-service binding must have its bookkeeping cleared"
    );
    assert_eq!(
        interrupts::pic::eoi_count_for_test(),
        0,
        "check_stale_bindings must never force an EOI for a line whose ISR bit isn't set"
    );

    sched::terminate_task(task_id);
    irq_bridge::reset_bindings_for_test();
}

/// Tests that `IrqAck` clears the watchdog's `isr_set_since_tsc` bookkeeping
/// for the acknowledged line, so `check_stale_bindings` never mistakes a
/// properly-acked occurrence for a stuck one.
#[test_case]
fn test_irq_ack_clears_stale_binding_watchdog_timestamp() {
    irq_bridge::reset_bindings_for_test();

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(task_id_slot(task_id)));
    dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0).expect("subscribe IRQ 11");

    let mut regs = SavedRegisters::default();
    driver_irq_trampoline(11, &mut regs);
    assert_ne!(
        irq_bridge::isr_set_since_for_test(11),
        Some(0),
        "the trampoline must record a watchdog timestamp when the IRQ fires"
    );

    dispatch_checked(SyscallId::IRQ_ACK, 11, 0, 0, 0).expect("ack IRQ 11");
    assert_eq!(
        irq_bridge::isr_set_since_for_test(11),
        Some(0),
        "IrqAck must clear the watchdog timestamp for the line it just acknowledged"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    irq_bridge::reset_bindings_for_test();
}

/// Tests that IrqSubscribe refuses to silently overwrite a kernel-internal
/// handler already registered for a vector — here, IRQ0's timer handler,
/// registered by `sched::init()` at boot (mirrors a real shared legacy PCI
/// line, e.g. a NIC sharing IRQ14 with the kernel's own `ata` handler).
#[test_case]
fn test_irq_subscribe_refuses_to_steal_kernel_handler() {
    irq_bridge::reset_bindings_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![0],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 0, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "IrqSubscribe must refuse to overwrite the kernel's own IRQ0 timer handler"
    );
    assert!(
        !irq_bridge::is_driver_irq(0),
        "a refused subscribe must not leave IRQ0 bound to the driver task"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
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

/// Tests that IrqWait actually honors its timeout instead of blocking forever
/// when no IRQ ever fires. Before the fix, `irq_bridge::wait`'s `timeout_ms`
/// parameter was ignored (`_timeout_ms`), so this exact scenario — subscribe,
/// then wait with no IRQ pending and none ever triggered via
/// `driver_irq_trampoline` — would hang the calling task (and this test)
/// forever instead of returning.
#[test_case]
fn test_irq_wait_times_out_when_no_irq_fires() {
    irq_bridge::reset_bindings_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![13],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let sub_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 13, 0, 0, 0);
    assert_eq!(
        sub_res,
        Ok(SYSCALL_OK),
        "Task should successfully subscribe to IRQ 13"
    );

    // No `driver_irq_trampoline(13, ...)` call anywhere in this test: the IRQ
    // never fires, so a bounded IrqWait must give up on its own.
    let wait_res = dispatch_checked(SyscallId::IRQ_WAIT, 13, 20, 0, 0);
    assert_eq!(
        wait_res,
        Err(SyscallError::Timeout),
        "IrqWait with a timeout and no IRQ must return Timeout instead of blocking forever"
    );

    // A timed-out wait must not disturb the caller's IRQ ownership — it may
    // simply call IrqWait again.
    assert!(
        irq_bridge::is_driver_irq(13),
        "a timed-out IrqWait must not release the caller's IRQ subscription"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    irq_bridge::reset_bindings_for_test();
}

/// Tests that terminating a driver task registered as its own IRQ's waiter
/// does not deadlock the scheduler.
///
/// `remove_task` runs inside `terminate_task`'s `with_scheduler` critical
/// section and calls `irq_bridge::release_task`, which wakes any task
/// registered on that IRQ's wait queue. `SCHED` is a non-reentrant spinlock,
/// so if that wake path re-acquired it (as the ordinary lock-acquiring
/// `unblock_task` would), this test would hang until the QEMU test-runner
/// timeout instead of completing.
#[test_case]
fn test_irq_release_task_does_not_deadlock_scheduler_lock() {
    irq_bridge::reset_bindings_for_test();
    let task1_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task 1");
    let slot1 = task_id_slot(task1_id);

    let grants1 = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![9],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps1_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants1)));
    set_task_caps(task1_id, caps1_ptr);

    set_running_slot_for_test(Some(slot1));
    let sub_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 9, 0, 0, 0);
    assert_eq!(
        sub_res,
        Ok(SYSCALL_OK),
        "Task 1 should successfully subscribe to IRQ 9"
    );

    // Simulate task1 being blocked inside an infinite-timeout IrqWait: it is
    // registered as the binding's waiter but has not consumed a pending IRQ.
    assert!(
        irq_bridge::register_waiter_for_test(9, task1_id),
        "test setup: task1 must be registered as IRQ 9's waiter"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task1_id);

    assert!(
        !irq_bridge::is_driver_irq(9),
        "IRQ 9 binding must be released even when the owner was a registered waiter"
    );

    irq_bridge::reset_bindings_for_test();
}
