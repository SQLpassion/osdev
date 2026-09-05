//! Integration tests for `lib_driver::client::NicClient` (Phase 2 Step 4 of
//! `docs/nic_driver_design.md`).
//!
//! `NicClient`'s methods are thin wrappers around raw `int 0x80` syscalls
//! (`lib_driver::drv`) and cannot be safely exercised in a host `cargo test`
//! run -- but this *kernel* integration test genuinely can call them: this
//! binary boots as its own tiny kernel (`interrupts::init()` wires up the
//! real `int 0x80` gate to `syscall::dispatch_checked`, see
//! `kernel/src/arch/interrupts/handlers.rs`'s `syscall_rust_dispatch`), so
//! `int 0x80` here is a real, correctly-handled software interrupt, not a
//! no-op or a fault -- exactly like a genuine syscall, just issued from ring
//! 0 instead of ring 3 (a lower-CPL caller may always invoke a gate with a
//! less-restrictive DPL). This lets the test call `lib_driver` directly
//! instead of going through `dispatch_checked`, so it exercises the exact
//! production code path (register marshaling,
//! `int 0x80`, `decode_result`) rather than re-implementing it.
//!
//! One consequence of this technique: every pointer argument the *test*
//! constructs (a name string, a packet buffer) must live in **user-mapped**
//! memory (via [`write_bytes_to_user_page`]/[`user_str`]) -- a plain Rust
//! string literal or stack local here lives on this kernel binary's own
//! kernel-half stack/rodata, which `is_valid_user_buffer[_readable]`
//! correctly rejects, exactly as it would for a real driver handing the
//! kernel a bogus pointer.
//!
//! `NicClient::query_status`/`lib_driver::drv::query_status` cannot be
//! exercised this same way: that function's destination buffer is an
//! internal `MaybeUninit<UserDriverStatus>` local the *caller* has no way to
//! relocate, and every "kernel task" in this harness runs on a kernel-half
//! stack (this is a higher-half kernel; `spawn_kernel_task` never allocates
//! a separate user address space). In genuine ring-3 execution that local
//! lives on the *real* user-mode stack (a canonical, user-mapped VA), so
//! `is_valid_user_buffer_writable` accepts it there -- the limitation is an
//! artifact of driving ring-3-oriented library code from a ring-0 test
//! harness, not a bug in `DrvQuery` itself, which `driver_status_test.rs`
//! (Step 3) already exercises exhaustively via `dispatch_checked` with a
//! genuinely user-mapped output buffer.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;
use core::panic::PanicInfo;

use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::registry;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler::{self as sched, set_running_slot_for_test, task_id_slot};

use lib_driver::client::NicClient;
use lib_driver::{drv, SysError};

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

/// Maps one fresh, writable user page at `USER_HEAP_BASE + offset` and copies
/// `bytes` to its start. Returns the user virtual address and the physical
/// frame backing it (for cleanup).
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

/// Copies `s` into a fresh user-mapped page and returns a `&str` view backed
/// by that mapping, so `NicClient::open` (and anything else that takes a
/// user-space `&str`/`&[u8]`) receives a genuinely user-space pointer
/// instead of this binary's own kernel-half rodata.
///
/// Leaks the mapping deliberately: each `#[test_case]` in this file cleans
/// up via [`release_user_page`] using the returned address once it is done.
fn user_str(offset: u64, s: &str) -> (&'static str, u64, u64) {
    let (va, phys) = write_bytes_to_user_page(offset, s.as_bytes());
    // SAFETY:
    // - `va` is mapped, readable, and now holds exactly `s.as_bytes()`.
    // - The bytes are valid UTF-8 because they are an exact copy of `s`.
    // - The mapping is released by the caller via `release_user_page`
    //   using the same `va`/`phys` returned here, no earlier than that.
    let str_ref = unsafe {
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(va as *const u8, s.len()))
    };
    (str_ref, va, phys)
}

/// `NicClient::open` resolves a registered driver's tid, and fails cleanly
/// for an unregistered name.
#[test_case]
fn test_nic_client_open_success_and_unknown_name() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    registry::register(b"nic:client-open", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (name, name_va, name_phys) = user_str(0x1000, "nic:client-open");
    assert!(NicClient::open(name).is_ok());

    let (unknown_name, unknown_va, unknown_phys) = user_str(0x2000, "nic:does-not-exist");
    let unknown = NicClient::open(unknown_name);
    match unknown {
        Err(e) => assert_eq!(e, SysError::InvalidArgument),
        Ok(_) => panic!("opening an unregistered name must fail"),
    }

    release_user_page(name_va, name_phys);
    release_user_page(unknown_va, unknown_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `NicClient::send` (called by an app) lands in the driver's TX ring; the
/// driver's own raw `drv::net_recv` on its own tid observes the same bytes.
#[test_case]
fn test_nic_client_send_delivers_to_driver_via_raw_net_recv() {
    registry::reset_for_test();
    let driver_task = sched::spawn_kernel_task(test_task_loop).expect("spawn driver task");
    registry::register(b"nic:client-send", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let driver_slot = task_id_slot(driver_task);
    let app_slot = task_id_slot(app_task);

    set_running_slot_for_test(Some(app_slot));
    let (name, name_va, name_phys) = user_str(0x3000, "nic:client-send");
    let client = NicClient::open(name).expect("open must succeed");
    let (frame_va, frame_phys) = write_bytes_to_user_page(0x4000, b"from-nic-client");
    // SAFETY: `frame_va` is mapped and holds exactly this payload.
    let frame = unsafe { core::slice::from_raw_parts(frame_va as *const u8, 15) };
    assert!(client.send(frame).is_ok());

    set_running_slot_for_test(Some(driver_slot));
    let (out_va, out_phys) = write_bytes_to_user_page(0x5000, &[0u8; 32]);
    // SAFETY: `out_va` is mapped writable for 32 bytes.
    let out_buf = unsafe { core::slice::from_raw_parts_mut(out_va as *mut u8, 32) };
    let n = drv::net_recv(driver_task as u64, out_buf, 0).expect("driver must see the app's send");
    assert_eq!(&out_buf[..n], b"from-nic-client");

    release_user_page(name_va, name_phys);
    release_user_page(frame_va, frame_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// The driver's own raw `drv::net_send` on its own tid lands in the RX ring;
/// `NicClient::recv` (called by an app) observes the same bytes.
#[test_case]
fn test_nic_client_recv_observes_raw_driver_net_send() {
    registry::reset_for_test();
    let driver_task = sched::spawn_kernel_task(test_task_loop).expect("spawn driver task");
    registry::register(b"nic:client-recv", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let driver_slot = task_id_slot(driver_task);
    let app_slot = task_id_slot(app_task);

    set_running_slot_for_test(Some(driver_slot));
    let (frame_va, frame_phys) = write_bytes_to_user_page(0x6000, b"from-driver-raw");
    // SAFETY: `frame_va` is mapped and holds exactly this payload.
    let frame = unsafe { core::slice::from_raw_parts(frame_va as *const u8, 15) };
    drv::net_send(driver_task as u64, frame).expect("driver's own NetSend must succeed");

    set_running_slot_for_test(Some(app_slot));
    let (name, name_va, name_phys) = user_str(0x7000, "nic:client-recv");
    let client = NicClient::open(name).expect("open must succeed");
    let (out_va, out_phys) = write_bytes_to_user_page(0x8000, &[0u8; 32]);
    // SAFETY: `out_va` is mapped writable for 32 bytes.
    let out_buf = unsafe { core::slice::from_raw_parts_mut(out_va as *mut u8, 32) };
    let n = client
        .recv(out_buf, 0)
        .expect("app must see the driver's raw send");
    assert_eq!(&out_buf[..n], b"from-driver-raw");

    release_user_page(name_va, name_phys);
    release_user_page(frame_va, frame_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// `NicClient::recv` with a short timeout and nothing queued returns
/// `Err(SysError::Timeout)` instead of hanging the test.
#[test_case]
fn test_nic_client_recv_timeout_does_not_hang() {
    registry::reset_for_test();
    let driver_task = sched::spawn_kernel_task(test_task_loop).expect("spawn driver task");
    registry::register(b"nic:client-timeout", driver_task).expect("register");
    let slot = task_id_slot(driver_task);
    set_running_slot_for_test(Some(slot));

    let (name, name_va, name_phys) = user_str(0x9000, "nic:client-timeout");
    let client = NicClient::open(name).expect("open must succeed");
    let (out_va, out_phys) = write_bytes_to_user_page(0xA000, &[0u8; 32]);
    // SAFETY: `out_va` is mapped writable for 32 bytes.
    let out_buf = unsafe { core::slice::from_raw_parts_mut(out_va as *mut u8, 32) };
    // No NetSend anywhere in this test: the ring never fills.
    let res = client.recv(out_buf, 20);
    assert_eq!(res, Err(SysError::Timeout));

    release_user_page(name_va, name_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    registry::reset_for_test();
}
