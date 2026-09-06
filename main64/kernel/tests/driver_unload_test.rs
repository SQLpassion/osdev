//! Integration tests for the `DrvUnload` / `DrvList` syscalls (issue #102).

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
use kaos_kernel::syscall::{
    dispatch_checked, syscall_name_for_number, SyscallError, SyscallId, UserDriverInfo,
};

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

/// Spawns a kernel test task with an attached `DriverCaps` block carrying
/// `flags`, so it passes whichever coarse capability check the caller wants
/// to exercise. Mirrors `driver_registry_test.rs`'s `spawn_driver_task`.
fn spawn_task_with_caps(flags: Capabilities) -> usize {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let grants = ResourceGrants {
        mmio_regions: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(flags, grants)));
    set_task_caps(task_id, caps_ptr);
    task_id
}

/// Maps one fresh, writable user page at `USER_HEAP_BASE + offset` and copies
/// `bytes` to its start. Returns the user virtual address the bytes were
/// written to, along with the physical frame backing it (for cleanup).
/// Mirrors `driver_registry_test.rs`'s helper of the same name.
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
// Debug names
// ---------------------------------------------------------------------------

#[test_case]
fn test_drv_unload_and_list_syscall_debug_names() {
    assert_eq!(syscall_name_for_number(SyscallId::DRV_UNLOAD), "DrvUnload");
    assert_eq!(syscall_name_for_number(SyscallId::DRV_LIST), "DrvList");
}

// ---------------------------------------------------------------------------
// DrvUnload
// ---------------------------------------------------------------------------

/// A task with no `DriverCaps` and no privileged flag may not call
/// `DrvUnload`, even with a well-formed (if unregistered) name — the
/// authorization check runs before the name pointer is ever touched.
#[test_case]
fn test_drv_unload_without_capability_or_privilege_fails() {
    registry::reset_for_test();

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let res = dispatch_checked(SyscallId::DRV_UNLOAD, 0, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "DrvUnload without UNLOAD_DRIVER capability must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvUnload` fails closed when there is no current task to authorize.
#[test_case]
fn test_drv_unload_with_no_current_task_fails_closed() {
    set_running_slot_for_test(None);

    let res = dispatch_checked(SyscallId::DRV_UNLOAD, 0, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "DrvUnload must fail closed, not open, when no task is currently running"
    );
}

/// A caller holding `UNLOAD_DRIVER` gets `InvalidArg` for a well-formed but
/// unregistered driver name, and the registry is left untouched.
#[test_case]
fn test_drv_unload_unknown_name_returns_invalid_arg() {
    registry::reset_for_test();

    let task_id = spawn_task_with_caps(Capabilities::UNLOAD_DRIVER);
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x1000, b"nic:does-not-exist");
    let res = dispatch_checked(
        SyscallId::DRV_UNLOAD,
        user_va,
        "nic:does-not-exist".len() as u64,
        0,
        0,
    );
    assert_eq!(res, Err(SyscallError::InvalidArg));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// End-to-end: an authorized caller unloads a registered driver by name —
/// the driver task is fully terminated (its slot freed) and its registry
/// entry disappears, so a subsequent `DrvLookup`-equivalent resolves to
/// nothing.
#[test_case]
fn test_drv_unload_end_to_end_terminates_driver_and_removes_registry_entry() {
    registry::reset_for_test();

    // The "driver" task: any live task id registered under a name is enough
    // — DrvUnload only needs a real, terminatable task, not a real device
    // binding.
    let driver_id = sched::spawn_kernel_task(test_task_loop).expect("spawn driver task");
    assert!(registry::register(b"nic:unload-me", driver_id).is_ok());

    // The caller: a separate task holding UNLOAD_DRIVER.
    let caller_id = spawn_task_with_caps(Capabilities::UNLOAD_DRIVER);
    let caller_slot = task_id_slot(caller_id);
    set_running_slot_for_test(Some(caller_slot));

    let (user_va, phys) = write_bytes_to_user_page(0x2000, b"nic:unload-me");
    let res = dispatch_checked(
        SyscallId::DRV_UNLOAD,
        user_va,
        "nic:unload-me".len() as u64,
        0,
        0,
    );
    assert!(res.is_ok(), "DrvUnload on a registered driver must succeed");

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);

    assert_eq!(
        registry::lookup(b"nic:unload-me"),
        None,
        "the driver's registry entry must be gone after DrvUnload"
    );
    assert_eq!(
        sched::task_state(driver_id),
        None,
        "the driver task itself must be fully terminated (slot freed) after DrvUnload"
    );

    sched::terminate_task(caller_id);
    registry::reset_for_test();
}

/// A privileged caller (no `DriverCaps` at all) is authorized the same way
/// `SpawnDriver` treats the boot shell — via the scheduler's privileged flag,
/// not a capability block.
#[test_case]
fn test_drv_unload_privileged_caller_without_caps_succeeds() {
    registry::reset_for_test();

    let driver_id = sched::spawn_kernel_task(test_task_loop).expect("spawn driver task");
    assert!(registry::register(b"nic:privileged-unload", driver_id).is_ok());

    let caller_id = sched::spawn_kernel_task(test_task_loop).expect("spawn caller task");
    assert!(
        sched::set_task_privileged_for_test(caller_id, true),
        "should mark caller task privileged"
    );
    let caller_slot = task_id_slot(caller_id);
    set_running_slot_for_test(Some(caller_slot));

    let (user_va, phys) = write_bytes_to_user_page(0x3000, b"nic:privileged-unload");
    let res = dispatch_checked(
        SyscallId::DRV_UNLOAD,
        user_va,
        "nic:privileged-unload".len() as u64,
        0,
        0,
    );
    assert!(
        res.is_ok(),
        "a privileged caller with no DriverCaps must still be authorized"
    );

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(caller_id);
    registry::reset_for_test();
}

// ---------------------------------------------------------------------------
// DrvList
// ---------------------------------------------------------------------------

/// `DrvList` reflects an empty registry, and requires no capability at all —
/// mirrors `DrvLookup`'s "read-only, ungated" contract. `max_entries == 0`
/// takes the fast path that never touches `out_ptr`, so a null pointer is
/// fine here.
#[test_case]
fn test_drv_list_empty_registry_requires_no_capability() {
    registry::reset_for_test();

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let res = dispatch_checked(SyscallId::DRV_LIST, 0, 0, 0, 0);
    assert_eq!(res, Ok(0));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvList` reflects exactly the currently registered drivers in one call,
/// and shrinks again once one is unloaded.
#[test_case]
fn test_drv_list_reflects_registered_drivers() {
    registry::reset_for_test();

    let driver_a = sched::spawn_kernel_task(test_task_loop).expect("spawn driver a");
    let driver_b = sched::spawn_kernel_task(test_task_loop).expect("spawn driver b");
    assert!(registry::register(b"nic:list-a", driver_a).is_ok());
    assert!(registry::register(b"nic:list-b", driver_b).is_ok());

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn caller");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    // A buffer big enough for both entries in a single call.
    let (out_va, out_phys) =
        write_bytes_to_user_page(0x4000, &[0u8; 2 * core::mem::size_of::<UserDriverInfo>()]);
    let total = dispatch_checked(SyscallId::DRV_LIST, out_va, 2, 0, 0)
        .expect("DrvList must succeed") as usize;
    assert_eq!(total, 2, "DrvList must report the total registered count");

    let mut seen_names = alloc::vec::Vec::new();
    for i in 0..total {
        // SAFETY: `out_va` was just written by the successful DrvList call
        // above for `total` entries, and each slot is large enough for a
        // `UserDriverInfo`.
        let info = unsafe { core::ptr::read_unaligned((out_va as *const UserDriverInfo).add(i)) };
        seen_names.push((
            info.name[..info.name_len as usize].to_vec(),
            info.tid as usize,
        ));
    }
    assert!(seen_names.contains(&(b"nic:list-a".to_vec(), driver_a)));
    assert!(seen_names.contains(&(b"nic:list-b".to_vec(), driver_b)));

    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);

    sched::terminate_task(driver_a);
    let count_after_unload = registry::list().len();
    assert_eq!(
        count_after_unload, 1,
        "the registry must shrink once a driver's owning task is terminated"
    );

    sched::terminate_task(driver_b);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `max_entries == 0` never dereferences `out_ptr`, even if it is a
/// completely bogus, non-canonical value — the fast path in
/// `syscall_drv_list_impl` must return before any pointer validation runs.
#[test_case]
fn test_drv_list_zero_max_entries_does_not_touch_out_ptr() {
    registry::reset_for_test();

    let driver_id = sched::spawn_kernel_task(test_task_loop).expect("spawn driver");
    assert!(registry::register(b"nic:list-zero-cap", driver_id).is_ok());

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let bogus_ptr = u64::MAX - 8; // non-canonical, would fault if dereferenced
    let res = dispatch_checked(SyscallId::DRV_LIST, bogus_ptr, 0, 0, 0);
    assert_eq!(
        res,
        Ok(1),
        "the total count must still be reported even with no buffer capacity"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    sched::terminate_task(driver_id);
    registry::reset_for_test();
}

/// `DrvList` rejects a canonical-but-unmapped output pointer once
/// `max_entries > 0` actually requires writing into it.
#[test_case]
fn test_drv_list_validates_out_ptr() {
    registry::reset_for_test();

    let driver_id = sched::spawn_kernel_task(test_task_loop).expect("spawn driver");
    assert!(registry::register(b"nic:list-ptr-check", driver_id).is_ok());

    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFD_0000;
    let res = dispatch_checked(SyscallId::DRV_LIST, unmapped_va, 1, 0, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    sched::terminate_task(driver_id);
    registry::reset_for_test();
}
