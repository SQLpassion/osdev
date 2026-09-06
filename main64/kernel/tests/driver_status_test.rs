//! Integration tests for driver status snapshots (`DrvPublishStatus`/`DrvQuery`,
//! Phase 2 Step 3 of `docs/nic_driver_design.md`).

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
    dispatch_checked, syscall_name_for_number, SyscallError, SyscallId, UserArpEntry,
    UserDriverStatus, MAX_ARP_ENTRIES,
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

/// Spawns a kernel test task with an attached (capability-empty) `DriverCaps`
/// block, so it passes `DrvRegister`'s "is a driver task" check.
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
/// `bytes` to its start (if non-empty). Returns the user virtual address and
/// the physical frame backing it (for cleanup).
fn write_bytes_to_user_page(offset: u64, bytes: &[u8]) -> (u64, u64) {
    assert!(bytes.len() <= 4096, "test helper only maps a single page");
    let user_va = vmm::USER_HEAP_BASE + offset;
    let phys = vmm::page_table::alloc_frame_phys().expect("alloc frame");
    let pfn = vmm::page_table::phys_to_pfn(phys);
    vmm::map_user_page(user_va, pfn, true).expect("map user page");
    if !bytes.is_empty() {
        // SAFETY:
        // - `user_va` was just mapped writable, one full page, by the call above.
        // - `bytes.len() <= 4096` guarantees the copy stays within that page.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), user_va as *mut u8, bytes.len());
        }
    }
    (user_va, phys)
}

/// Releases a page mapped by [`write_bytes_to_user_page`].
fn release_user_page(user_va: u64, phys: u64) {
    vmm::unmap_without_release(user_va);
    let pfn = vmm::page_table::phys_to_pfn(phys);
    pmm::with_pmm(|mgr| mgr.release_pfn(pfn));
}

/// Builds a sample `UserDriverStatus` with `arp_entry_count` synthetic
/// entries (`ip = [0,0,0,i]`, `mac = [0,0,0,0,0,i]`).
fn sample_status(arp_entry_count: u32) -> UserDriverStatus {
    let mut arp_entries = [UserArpEntry {
        ip: [0; 4],
        mac: [0; 6],
        _padding: [0; 2],
    }; MAX_ARP_ENTRIES];
    for (i, entry) in arp_entries
        .iter_mut()
        .enumerate()
        .take(arp_entry_count as usize)
    {
        entry.ip = [10, 0, 2, i as u8];
        entry.mac = [0x52, 0x54, 0x00, 0x12, 0x34, i as u8];
    }
    UserDriverStatus {
        mac: [0x52, 0x54, 0x00, 0x00, 0x00, 0x01],
        _padding0: [0; 2],
        ip: [10, 0, 2, 15],
        subnet_mask: [255, 255, 255, 0],
        gateway: [10, 0, 2, 2],
        dns: [8, 8, 8, 8],
        rx_packets: 42,
        rx_bytes: 4242,
        tx_packets: 24,
        tx_bytes: 2424,
        link_up: 1,
        _padding1: [0; 3],
        arp_entry_count,
        arp_entries,
    }
}

fn write_status_to_user_page(offset: u64, status: &UserDriverStatus) -> (u64, u64) {
    // SAFETY: reinterprets a `Copy`, `#[repr(C)]` struct as its raw bytes
    // purely to reuse the byte-oriented `write_bytes_to_user_page` helper.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (status as *const UserDriverStatus) as *const u8,
            core::mem::size_of::<UserDriverStatus>(),
        )
    };
    write_bytes_to_user_page(offset, bytes)
}

// ---------------------------------------------------------------------------
// registry:: tests (bypass the syscall boundary).
// ---------------------------------------------------------------------------

/// `registry::publish_status` succeeds for a registered driver, and
/// `registry::query_status` returns the exact snapshot back.
#[test_case]
fn test_registry_publish_and_query_round_trip() {
    registry::reset_for_test();
    let tid = 111;
    registry::register(b"nic:status-roundtrip", tid).expect("register");

    let status = sample_status(3);
    assert!(registry::publish_status(tid, status).is_ok());
    assert_eq!(registry::query_status(tid), Some(status));

    registry::reset_for_test();
}

/// `registry::publish_status` fails for a tid with no `DriverEntry`.
#[test_case]
fn test_registry_publish_status_unregistered_tid_fails() {
    registry::reset_for_test();
    assert_eq!(
        registry::publish_status(999_999, sample_status(0)),
        Err(SyscallError::InvalidArg)
    );
}

/// `registry::query_status` returns `None` for a registered driver that has
/// never published, and for an unknown tid.
#[test_case]
fn test_registry_query_status_none_cases() {
    registry::reset_for_test();
    let tid = 222;
    registry::register(b"nic:never-published", tid).expect("register");

    assert_eq!(registry::query_status(tid), None, "never published yet");
    assert_eq!(registry::query_status(999_999), None, "unknown tid");

    registry::reset_for_test();
}

// ---------------------------------------------------------------------------
// syscall_drv_publish_status_impl / syscall_drv_query_impl tests.
// ---------------------------------------------------------------------------

/// A caller with no `DriverEntry` for its own tid (never called
/// `DrvRegister`) may not call `DrvPublishStatus`.
#[test_case]
fn test_drv_publish_status_syscall_requires_registration() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let status = sample_status(0);
    let (status_va, status_phys) = write_status_to_user_page(0x1000, &status);
    let res = dispatch_checked(SyscallId::DRV_PUBLISH_STATUS, status_va, 0, 0, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    release_user_page(status_va, status_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvPublishStatus` rejects a canonical-but-unmapped `status_ptr`.
#[test_case]
fn test_drv_publish_status_syscall_validates_status_ptr() {
    registry::reset_for_test();
    let task_id = spawn_driver_task();
    registry::register(b"nic:publish-validate", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFB_0000;
    let res = dispatch_checked(SyscallId::DRV_PUBLISH_STATUS, unmapped_va, 0, 0, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `DrvPublishStatus` rejects `arp_entry_count > MAX_ARP_ENTRIES` and stores
/// no partial/garbage snapshot; a subsequent `DrvQuery` still sees "never
/// published" (`InvalidArg`), not the rejected data.
#[test_case]
fn test_drv_publish_status_syscall_rejects_arp_overflow() {
    registry::reset_for_test();
    let task_id = spawn_driver_task();
    registry::register(b"nic:arp-overflow", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let bad_status = sample_status((MAX_ARP_ENTRIES + 1) as u32);
    let (status_va, status_phys) = write_status_to_user_page(0x2000, &bad_status);
    let publish_res = dispatch_checked(SyscallId::DRV_PUBLISH_STATUS, status_va, 0, 0, 0);
    assert_eq!(publish_res, Err(SyscallError::InvalidArg));

    let (out_va, out_phys) = write_bytes_to_user_page(0x3000, &[0u8; 4]); // dummy, DrvQuery below is what matters
    let query_res = dispatch_checked(SyscallId::DRV_QUERY, task_id as u64, out_va, 0, 0);
    assert_eq!(
        query_res,
        Err(SyscallError::InvalidArg),
        "rejected publish must not leave a stored snapshot behind"
    );

    release_user_page(status_va, status_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// A valid publish followed by `DrvQuery` returns byte-identical field
/// values, including a non-trivial ARP table and the boundary case of
/// exactly `MAX_ARP_ENTRIES` entries.
#[test_case]
fn test_drv_publish_query_syscalls_round_trip_with_arp_table() {
    registry::reset_for_test();
    let driver_task = spawn_driver_task();
    registry::register(b"nic:round-trip", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let driver_slot = task_id_slot(driver_task);
    let app_slot = task_id_slot(app_task);

    for &count in &[3u32, MAX_ARP_ENTRIES as u32] {
        let status = sample_status(count);

        set_running_slot_for_test(Some(driver_slot));
        let (status_va, status_phys) = write_status_to_user_page(0x4000, &status);
        let publish_res = dispatch_checked(SyscallId::DRV_PUBLISH_STATUS, status_va, 0, 0, 0);
        assert!(publish_res.is_ok());

        set_running_slot_for_test(Some(app_slot));
        let (out_va, out_phys) =
            write_bytes_to_user_page(0x5000, &[0u8; core::mem::size_of::<UserDriverStatus>()]);
        let query_res = dispatch_checked(SyscallId::DRV_QUERY, driver_task as u64, out_va, 0, 0);
        assert!(query_res.is_ok());

        // SAFETY: `out_va` was mapped writable and DrvQuery copied a full
        // UserDriverStatus into it on success.
        let received = unsafe { core::ptr::read_unaligned(out_va as *const UserDriverStatus) };
        assert_eq!(received, status, "arp_entry_count = {}", count);

        release_user_page(status_va, status_phys);
        release_user_page(out_va, out_phys);
    }

    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// `DrvQuery` rejects a canonical-but-unmapped `out_ptr`.
#[test_case]
fn test_drv_query_syscall_validates_out_ptr() {
    registry::reset_for_test();
    let driver_task = spawn_driver_task();
    registry::register(b"nic:query-validate", driver_task).expect("register");
    let slot = task_id_slot(driver_task);
    set_running_slot_for_test(Some(slot));

    let status = sample_status(1);
    let (status_va, status_phys) = write_status_to_user_page(0x6000, &status);
    assert!(dispatch_checked(SyscallId::DRV_PUBLISH_STATUS, status_va, 0, 0, 0).is_ok());

    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFA_0000;
    let res = dispatch_checked(SyscallId::DRV_QUERY, driver_task as u64, unmapped_va, 0, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    release_user_page(status_va, status_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    registry::reset_for_test();
}

/// `DrvQuery` against an unknown `driver_id` fails.
#[test_case]
fn test_drv_query_syscall_unknown_driver_id() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (out_va, out_phys) =
        write_bytes_to_user_page(0x7000, &[0u8; core::mem::size_of::<UserDriverStatus>()]);
    let res = dispatch_checked(SyscallId::DRV_QUERY, 999_999, out_va, 0, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// If the publishing driver exits, its status snapshot is torn down along
/// with the rest of its `DriverEntry` (Step 1's `registry::release_task`) --
/// a subsequent `DrvQuery` fails rather than returning a stale snapshot.
#[test_case]
fn test_drv_query_fails_after_publishing_driver_exits() {
    registry::reset_for_test();
    let driver_task = spawn_driver_task();
    registry::register(b"nic:exits-after-publish", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let driver_slot = task_id_slot(driver_task);
    let app_slot = task_id_slot(app_task);

    set_running_slot_for_test(Some(driver_slot));
    let status = sample_status(2);
    let (status_va, status_phys) = write_status_to_user_page(0x8000, &status);
    assert!(dispatch_checked(SyscallId::DRV_PUBLISH_STATUS, status_va, 0, 0, 0).is_ok());

    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);

    set_running_slot_for_test(Some(app_slot));
    let (out_va, out_phys) =
        write_bytes_to_user_page(0x9000, &[0u8; core::mem::size_of::<UserDriverStatus>()]);
    let res = dispatch_checked(SyscallId::DRV_QUERY, driver_task as u64, out_va, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::InvalidArg),
        "DrvQuery must fail once the publishing driver has exited"
    );

    release_user_page(status_va, status_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// `syscall_name_for_number` resolves both new syscall numbers.
#[test_case]
fn test_drv_status_syscall_debug_names() {
    assert_eq!(
        syscall_name_for_number(SyscallId::DRV_PUBLISH_STATUS),
        "DrvPublishStatus"
    );
    assert_eq!(syscall_name_for_number(SyscallId::DRV_QUERY), "DrvQuery");
}
