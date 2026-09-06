//! Integration tests for the kernel DriverRegistry (`DrvRegister` / `DrvLookup`).

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
use kaos_kernel::drivers::registry;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
use kaos_kernel::scheduler::{
    self as sched, set_running_slot_for_test, set_task_caps, task_id_slot,
};
use kaos_kernel::syscall::{dispatch_checked, syscall_name_for_number, SyscallError, SyscallId};

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

/// Spawns a kernel test task with an attached (capability-empty) `DriverCaps`
/// block, so it passes `DrvRegister`'s "is a driver task" check — mirroring
/// how `driver_rtl8139_test.rs` attaches caps for its own syscall tests.
fn spawn_driver_task() -> usize {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let grants = ResourceGrants {
        mmio_regions: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::NONE, grants)));
    set_task_caps(task_id, caps_ptr);
    task_id
}

/// Maps one fresh, writable user page at `USER_HEAP_BASE + offset` and copies
/// `bytes` to its start. Returns the user virtual address the bytes were
/// written to, along with the physical frame backing it (for cleanup).
///
/// Mirrors the mapping pattern in
/// `test_rtl8139_driver_dma_allocation_and_translation`.
fn write_bytes_to_user_page(offset: u64, bytes: &[u8]) -> (u64, u64) {
    assert!(bytes.len() <= 4096, "test helper only maps a single page");
    let user_va = vmm::USER_HEAP_BASE + offset;
    let phys = vmm::page_table::alloc_frame_phys().expect("alloc frame");
    let pfn = vmm::page_table::phys_to_pfn(phys);
    vmm::map_user_page(user_va, pfn, true).expect("map user page");
    // SAFETY:
    // - `user_va` was just mapped writable, one full page, by the call above.
    // - `bytes.len() <= 4096` guarantees the copy stays within that page.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), user_va as *mut u8, bytes.len());
    }
    (user_va, phys)
}

/// Releases a page mapped by [`write_bytes_to_user_page`].
fn release_user_page(user_va: u64, phys: u64) {
    vmm::unmap_without_release(user_va);
    let pfn = vmm::page_table::phys_to_pfn(phys);
    pmm::with_pmm(|mgr| mgr.release_pfn(pfn));
}

// ---------------------------------------------------------------------------
// registry:: unit-level tests (bypass the syscall boundary entirely)
// ---------------------------------------------------------------------------

/// `registry::register` succeeds and `registry::lookup` resolves the exact
/// tid it was registered with; an unregistered name resolves to `None`.
#[test_case]
fn test_registry_register_success_and_lookup() {
    registry::reset_for_test();

    assert!(registry::register(b"nic:test-success", 42).is_ok());
    assert_eq!(registry::lookup(b"nic:test-success"), Some(42));
    assert_eq!(registry::lookup(b"nic:does-not-exist"), None);

    registry::reset_for_test();
}

/// A name longer than `DRIVER_NAME_LEN` is rejected.
#[test_case]
fn test_registry_register_rejects_name_too_long() {
    registry::reset_for_test();

    let too_long = [b'a'; registry::DRIVER_NAME_LEN + 1];
    assert_eq!(
        registry::register(&too_long, 1),
        Err(SyscallError::InvalidArg)
    );
    assert_eq!(
        registry::lookup(&too_long[..registry::DRIVER_NAME_LEN]),
        None
    );

    registry::reset_for_test();
}

/// The registry rejects a new registration once it holds `MAX_DRIVERS`
/// entries, without disturbing any existing entry.
#[test_case]
fn test_registry_register_rejects_when_full() {
    registry::reset_for_test();

    for i in 0..registry::MAX_DRIVERS {
        let name = alloc::format!("nic:cap-{}", i);
        assert!(
            registry::register(name.as_bytes(), 100 + i).is_ok(),
            "registration {} should succeed while under capacity",
            i
        );
    }

    // One more, past capacity, must fail.
    assert_eq!(
        registry::register(b"nic:overflow", 999),
        Err(SyscallError::InvalidArg)
    );

    // Every one of the first MAX_DRIVERS entries must still resolve.
    for i in 0..registry::MAX_DRIVERS {
        let name = alloc::format!("nic:cap-{}", i);
        assert_eq!(registry::lookup(name.as_bytes()), Some(100 + i));
    }
    assert_eq!(registry::lookup(b"nic:overflow"), None);

    registry::reset_for_test();
}

/// Registering an already-registered name fails and leaves the first
/// registration untouched.
#[test_case]
fn test_registry_register_rejects_duplicate_name() {
    registry::reset_for_test();

    assert!(registry::register(b"nic:dup", 1).is_ok());
    assert_eq!(
        registry::register(b"nic:dup", 2),
        Err(SyscallError::InvalidArg)
    );
    assert_eq!(
        registry::lookup(b"nic:dup"),
        Some(1),
        "first registration must remain untouched"
    );

    registry::reset_for_test();
}

/// `registry::release_task` removes only the entry matching the given tid,
/// and is a no-op when no entry matches.
#[test_case]
fn test_registry_release_task_removes_matching_entries_only() {
    registry::reset_for_test();

    assert!(registry::register(b"nic:owned-by-10", 10).is_ok());
    assert!(registry::register(b"nic:owned-by-20", 20).is_ok());

    // No-op for an unrelated tid.
    registry::release_task(12345);
    assert_eq!(registry::lookup(b"nic:owned-by-10"), Some(10));
    assert_eq!(registry::lookup(b"nic:owned-by-20"), Some(20));

    // Releases only the matching entry.
    registry::release_task(10);
    assert_eq!(registry::lookup(b"nic:owned-by-10"), None);
    assert_eq!(registry::lookup(b"nic:owned-by-20"), Some(20));

    registry::reset_for_test();
}

/// `syscall_name_for_number` resolves both new syscall numbers to their
/// stable debug names, matching the pattern already asserted for every
/// lower-numbered syscall in `syscall_dispatch_test.rs`.
#[test_case]
fn test_drv_syscall_debug_names() {
    assert_eq!(
        syscall_name_for_number(SyscallId::DRV_REGISTER),
        "DrvRegister"
    );
    assert_eq!(syscall_name_for_number(SyscallId::DRV_LOOKUP), "DrvLookup");
}

// ---------------------------------------------------------------------------
// syscall_drv_register_impl / syscall_drv_lookup_impl tests (via dispatch_checked)
// ---------------------------------------------------------------------------

/// A task with no `DriverCaps` (an ordinary Ring-3 app) may not call
/// `DrvRegister`.
#[test_case]
fn test_drv_register_syscall_requires_driver_caps() {
    registry::reset_for_test();

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x1000, b"nic:no-caps");
    let res = dispatch_checked(SyscallId::DRV_REGISTER, user_va, 11, 0, 0);
    assert_eq!(res, Err(SyscallError::PermissionDenied));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvRegister` rejects a zero length and a length exceeding
/// `DRIVER_NAME_LEN`, without ever registering anything.
#[test_case]
fn test_drv_register_syscall_validates_name_len() {
    registry::reset_for_test();

    let task_id = spawn_driver_task();
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x2000, b"nic:whatever");

    let zero_len = dispatch_checked(SyscallId::DRV_REGISTER, user_va, 0, 0, 0);
    assert_eq!(zero_len, Err(SyscallError::InvalidArg));

    let too_long = dispatch_checked(
        SyscallId::DRV_REGISTER,
        user_va,
        (registry::DRIVER_NAME_LEN + 1) as u64,
        0,
        0,
    );
    assert_eq!(too_long, Err(SyscallError::InvalidArg));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvRegister` rejects a name pointer that is canonical but not actually
/// mapped in the caller's address space.
#[test_case]
fn test_drv_register_syscall_validates_name_ptr() {
    registry::reset_for_test();

    let task_id = spawn_driver_task();
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    // A canonical user address that this test never mapped.
    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFF_0000;
    let res = dispatch_checked(SyscallId::DRV_REGISTER, unmapped_va, 8, 0, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// A successful `DrvRegister` call registers exactly the bytes supplied,
/// under the calling task's packed id.
#[test_case]
fn test_drv_register_syscall_success_registers_exact_bytes() {
    registry::reset_for_test();

    let task_id = spawn_driver_task();
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));
    let expected_tid = sched::current_task_id().expect("running slot is set");

    let name = b"nic:success-path";
    let (user_va, phys) = write_bytes_to_user_page(0x3000, name);
    let res = dispatch_checked(SyscallId::DRV_REGISTER, user_va, name.len() as u64, 0, 0);
    assert!(res.is_ok());
    assert_eq!(registry::lookup(name), Some(expected_tid));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvLookup` requires no capability at all — a task with no `DriverCaps`
/// can still resolve a name registered by another task.
#[test_case]
fn test_drv_lookup_syscall_no_capability_required() {
    registry::reset_for_test();
    assert!(registry::register(b"nic:lookup-target", 777).is_ok());

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x4000, b"nic:lookup-target");
    let res = dispatch_checked(
        SyscallId::DRV_LOOKUP,
        user_va,
        "nic:lookup-target".len() as u64,
        0,
        0,
    );
    assert_eq!(res, Ok(777));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvLookup` shares the same length/pointer validation as `DrvRegister`,
/// and returns `InvalidArg` for a well-formed but unregistered name.
#[test_case]
fn test_drv_lookup_syscall_validates_name_len_and_ptr_and_unknown_name() {
    registry::reset_for_test();

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x5000, b"nic:unregistered");

    let zero_len = dispatch_checked(SyscallId::DRV_LOOKUP, user_va, 0, 0, 0);
    assert_eq!(zero_len, Err(SyscallError::InvalidArg));

    let too_long = dispatch_checked(
        SyscallId::DRV_LOOKUP,
        user_va,
        (registry::DRIVER_NAME_LEN + 1) as u64,
        0,
        0,
    );
    assert_eq!(too_long, Err(SyscallError::InvalidArg));

    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFE_0000;
    let bad_ptr = dispatch_checked(SyscallId::DRV_LOOKUP, unmapped_va, 8, 0, 0);
    assert_eq!(bad_ptr, Err(SyscallError::InvalidArg));

    let unknown = dispatch_checked(
        SyscallId::DRV_LOOKUP,
        user_va,
        "nic:unregistered".len() as u64,
        0,
        0,
    );
    assert_eq!(unknown, Err(SyscallError::InvalidArg));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

// ---------------------------------------------------------------------------
// End-to-end: register from one task, resolve from another, verify cleanup
// on exit (exercises the `remove_task` hook, not just the registry function).
// ---------------------------------------------------------------------------

/// Task A registers a name; task B resolves it via `DrvLookup`. Once task A
/// exits, `remove_task`'s cleanup hook must remove its registration, so a
/// subsequent `DrvLookup` for the same name fails.
#[test_case]
fn test_drv_register_lookup_end_to_end_and_cleanup_on_exit() {
    registry::reset_for_test();

    // Task A registers "nic:e2e".
    let task_a = spawn_driver_task();
    let slot_a = task_id_slot(task_a);
    set_running_slot_for_test(Some(slot_a));
    let tid_a = sched::current_task_id().expect("running slot is set");

    let name = b"nic:e2e";
    let (reg_va, reg_phys) = write_bytes_to_user_page(0x6000, name);
    let reg_res = dispatch_checked(SyscallId::DRV_REGISTER, reg_va, name.len() as u64, 0, 0);
    assert!(reg_res.is_ok());
    release_user_page(reg_va, reg_phys);
    set_running_slot_for_test(None);

    // Task B resolves "nic:e2e" and gets task A's packed id back.
    let task_b = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot_b = task_id_slot(task_b);
    set_running_slot_for_test(Some(slot_b));

    let (lookup_va, lookup_phys) = write_bytes_to_user_page(0x7000, name);
    let lookup_res = dispatch_checked(SyscallId::DRV_LOOKUP, lookup_va, name.len() as u64, 0, 0);
    assert_eq!(lookup_res, Ok(tid_a as u64));

    // Task A exits. `remove_task` must release its DriverRegistry entry.
    set_running_slot_for_test(None);
    sched::terminate_task(task_a);

    set_running_slot_for_test(Some(slot_b));
    let lookup_after_exit =
        dispatch_checked(SyscallId::DRV_LOOKUP, lookup_va, name.len() as u64, 0, 0);
    assert_eq!(
        lookup_after_exit,
        Err(SyscallError::InvalidArg),
        "DrvLookup must fail once the registering task has exited"
    );

    release_user_page(lookup_va, lookup_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_b);
    registry::reset_for_test();
}
