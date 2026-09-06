//! Integration test for the background driver's TX/RX/status round trip
//! (Phase 2 Step 6 of `docs/nic_driver_design.md`).
//!
//! `lib_driver_runtime::run_background_driver` itself is **not** called
//! directly here. Its `tx_buf`/`rx_buf` are local stack arrays inside a
//! function that would run on a "kernel task"'s stack in this harness
//! (`spawn_kernel_task` never allocates a separate user address space, since
//! this is a higher-half kernel) -- a kernel-half address, which
//! `is_valid_user_buffer_readable`/`_writable` correctly rejects on every
//! `NetSend`/`NetRecv`/`DrvPublishStatus` call the function would make. This
//! is the exact same structural limitation `net_client_test.rs` (Step 4)
//! documents for `NicClient::query_status`: in genuine ring-3 execution
//! those buffers live on the real, user-mapped stack and this is a non-issue
//! -- only driving ring-3-oriented library code from a ring-0 test harness
//! hits it. Building a full ring-3 ELF test harness just to exercise this
//! one function's own stack layout is out of proportion for what would
//! otherwise be tested.
//!
//! Instead, this test manually replicates `run_background_driver`'s exact
//! steps (register + resolve own id, drain app->driver sends, forward
//! driver->app sends, publish + query status) using the same
//! `lib_driver::drv` calls that function uses internally, with every buffer
//! properly user-mapped -- exercising the identical kernel-side code paths
//! (`NetSend`/`NetRecv`'s role-based rings, `DrvPublishStatus`/`DrvQuery`)
//! `run_background_driver` depends on.
//!
//! The status snapshot itself is built by hand rather than via
//! `lib_driver_runtime::build_status` (already covered by that crate's own
//! host tests): `lib_driver_runtime` depends on `lib_kaos`, which registers
//! its own `#[global_allocator]` -- adding it as a kernel dev-dependency
//! conflicts with the kernel's own allocator (`kernel/src/allocator.rs`).

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
use kaos_kernel::syscall::{dispatch_checked, SyscallId};

use lib_driver::drv;
use lib_driver::UserArpEntry;

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

fn release_user_page(user_va: u64, phys: u64) {
    vmm::unmap_without_release(user_va);
    let pfn = vmm::page_table::phys_to_pfn(phys);
    pmm::with_pmm(|mgr| mgr.release_pfn(pfn));
}

/// Copies `bytes` into a fresh user-mapped page and returns a `&[u8]` view
/// backed by that mapping, so syscall arguments the test constructs (e.g. a
/// driver name) are genuinely user-space pointers instead of this binary's
/// own kernel-half rodata/stack -- a plain `b"..."` literal here lives at a
/// kernel-half address, which `is_valid_user_buffer_readable` correctly
/// rejects (see `net_client_test.rs`'s `user_str` for the identical need).
fn user_bytes(offset: u64, bytes: &[u8]) -> (&'static [u8], u64, u64) {
    let (va, phys) = write_bytes_to_user_page(offset, bytes);
    // SAFETY: `va` is mapped, readable, and now holds exactly `bytes`,
    // released no earlier than the matching `release_user_page(va, phys)`.
    let slice = unsafe { core::slice::from_raw_parts(va as *const u8, bytes.len()) };
    (slice, va, phys)
}

/// Full round trip: `DrvRegister` + `DrvLookup` of one's own name (exactly
/// `run_background_driver`'s Step 0), an app's `NetSend` observed by the
/// driver's `NetRecv` (Step 1: "drain app -> driver TX requests"), the
/// driver's `NetSend` observed by the app's `NetRecv` (Step 2: "forward a
/// frame picked up from the wire"), and `build_status`'s output round-
/// tripping through `DrvPublishStatus`/`DrvQuery` (Step 3), including a
/// plausible MAC and RX/TX counters -- covering this issue's acceptance
/// criteria end to end.
#[test_case]
fn test_background_driver_tx_rx_and_status_round_trip() {
    registry::reset_for_test();

    let driver_task = spawn_driver_task();
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let driver_slot = task_id_slot(driver_task);
    let app_slot = task_id_slot(app_task);

    // Step 0: the driver registers itself and resolves its own packed id,
    // exactly as run_background_driver's Step 0 does. The name must live in
    // user-mapped memory -- a plain b"..." literal lives in this binary's
    // own kernel-half rodata, which DrvRegister/DrvLookup's buffer
    // validation correctly rejects.
    set_running_slot_for_test(Some(driver_slot));
    let (name_bytes, name_va, name_phys) = user_bytes(0x1000, b"nic:bg-loop-test");
    drv::drv_register(name_bytes).expect("DrvRegister must succeed");
    let own_id = drv::drv_lookup(name_bytes).expect("DrvLookup of own name must succeed");
    assert_eq!(own_id, driver_task as u64);

    // Step 1: an app sends a frame; the driver's own-tid NetRecv (what
    // run_background_driver's TX-drain loop calls before transmitting)
    // observes it.
    set_running_slot_for_test(Some(app_slot));
    let (app_frame, app_frame_va, app_frame_phys) = user_bytes(0x2000, b"app-to-wire");
    drv::net_send(own_id, app_frame).expect("app NetSend must succeed");

    set_running_slot_for_test(Some(driver_slot));
    let (tx_buf_va, tx_buf_phys) = write_bytes_to_user_page(0x3000, &[0u8; 64]);
    // SAFETY: `tx_buf_va` is mapped writable for 64 bytes.
    let tx_buf = unsafe { core::slice::from_raw_parts_mut(tx_buf_va as *mut u8, 64) };
    let n = drv::net_recv(own_id, tx_buf, 0).expect("driver NetRecv must see the app's send");
    assert_eq!(&tx_buf[..n], b"app-to-wire");

    // Step 2: the driver "picked up a frame from the wire" and forwards it
    // (own-tid NetSend, landing in its RX ring); the app's NetRecv observes it.
    let (wire_frame, wire_frame_va, wire_frame_phys) = user_bytes(0x4000, b"wire-to-app");
    drv::net_send(own_id, wire_frame).expect("driver NetSend must succeed");

    set_running_slot_for_test(Some(app_slot));
    let (rx_buf_va, rx_buf_phys) = write_bytes_to_user_page(0x5000, &[0u8; 64]);
    // SAFETY: `rx_buf_va` is mapped writable for 64 bytes.
    let rx_buf = unsafe { core::slice::from_raw_parts_mut(rx_buf_va as *mut u8, 64) };
    let n = drv::net_recv(own_id, rx_buf, 0).expect("app NetRecv must see the driver's send");
    assert_eq!(&rx_buf[..n], b"wire-to-app");

    // Step 3: a status snapshot shaped exactly like
    // lib_driver_runtime::build_status's output (a plausible MAC and RX/TX
    // counters) round-trips through DrvPublishStatus/DrvQuery. Built by hand
    // here rather than via build_status itself -- see this file's header
    // comment for why lib_driver_runtime can't be a kernel dev-dependency.
    //
    // Both the published struct and the query's output buffer must live in
    // user-mapped memory: `drv::publish_status` takes `&UserDriverStatus`,
    // which for a plain local would be this task's own kernel-half stack;
    // and `drv::query_status`'s internal `MaybeUninit` destination has that
    // same problem with no way for this caller to relocate it (identical to
    // `net_client_test.rs`'s NicClient::query_status limitation), so the
    // query side goes through `dispatch_checked` directly with an explicit
    // user-mapped output buffer instead, mirroring driver_status_test.rs.
    set_running_slot_for_test(Some(driver_slot));
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let status = lib_driver::UserDriverStatus {
        mac,
        _padding0: [0; 2],
        ip: [10, 0, 2, 15],
        subnet_mask: [255, 255, 255, 0],
        gateway: [10, 0, 2, 2],
        dns: [8, 8, 8, 8],
        rx_packets: 7,
        rx_bytes: 700,
        tx_packets: 3,
        tx_bytes: 300,
        link_up: 1,
        _padding1: [0; 3],
        arp_entry_count: 0,
        arp_entries: [UserArpEntry {
            ip: [0; 4],
            mac: [0; 6],
            _padding: [0; 2],
        }; lib_driver::MAX_ARP_ENTRIES],
    };
    // SAFETY: reinterprets a Copy, #[repr(C)] struct as its raw bytes purely
    // to reuse the byte-oriented write_bytes_to_user_page helper.
    let status_bytes = unsafe {
        core::slice::from_raw_parts(
            (&status as *const lib_driver::UserDriverStatus) as *const u8,
            core::mem::size_of::<lib_driver::UserDriverStatus>(),
        )
    };
    let (status_va, status_phys) = write_bytes_to_user_page(0x6000, status_bytes);
    // SAFETY: `status_va` now holds an exact byte copy of `status`, a valid
    // #[repr(C)] UserDriverStatus, and stays mapped until release_user_page
    // below runs.
    let status_ref = unsafe { &*(status_va as *const lib_driver::UserDriverStatus) };
    drv::publish_status(status_ref).expect("DrvPublishStatus must succeed");

    set_running_slot_for_test(Some(app_slot));
    let (query_out_va, query_out_phys) = write_bytes_to_user_page(
        0x7000,
        &[0u8; core::mem::size_of::<lib_driver::UserDriverStatus>()],
    );
    let query_res = dispatch_checked(SyscallId::DRV_QUERY, own_id, query_out_va, 0, 0);
    assert!(query_res.is_ok(), "DrvQuery must succeed");
    // SAFETY: `query_out_va` was mapped writable and DrvQuery copied a full
    // status snapshot into it before returning.
    let queried =
        unsafe { core::ptr::read_unaligned(query_out_va as *const lib_driver::UserDriverStatus) };
    assert_eq!(queried.mac, mac);
    assert_eq!(queried.rx_packets, 7);
    assert_eq!(queried.rx_bytes, 700);
    assert_eq!(queried.tx_packets, 3);
    assert_eq!(queried.tx_bytes, 300);
    assert_eq!(queried.link_up, 1);

    release_user_page(name_va, name_phys);
    release_user_page(app_frame_va, app_frame_phys);
    release_user_page(tx_buf_va, tx_buf_phys);
    release_user_page(wire_frame_va, wire_frame_phys);
    release_user_page(rx_buf_va, rx_buf_phys);
    release_user_page(status_va, status_phys);
    release_user_page(query_out_va, query_out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}
