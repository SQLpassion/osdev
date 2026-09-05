//! Integration tests for per-driver packet rings and the `NetSend`/`NetRecv`
//! syscalls (Phase 2 Step 2 of `docs/nic_driver_design.md`).
//!
//! The genuine two-task "blocked NetRecv is woken by another task's NetSend"
//! scenario needs real preemptive scheduling (an orchestrator task + a
//! running timer), which does not mix with this file's simpler
//! `set_running_slot_for_test`-driven style — see `net_ring_wakeup_test.rs`
//! for that one test, modeled on `fat32_concurrent_test.rs`.

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
/// block, so it passes `DrvRegister`'s "is a driver task" check.
fn spawn_driver_task() -> usize {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![],
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

// ---------------------------------------------------------------------------
// registry:: ring tests (bypass the syscall boundary; exercise push/pop
// directly through the public push_packet/try_pop_packet API).
// ---------------------------------------------------------------------------

/// An app pushes (TX ring); the driver pops from its own tid and gets the
/// same bytes back.
#[test_case]
fn test_registry_push_pop_round_trip_tx_direction() {
    registry::reset_for_test();
    let tid = 111;
    registry::register(b"nic:tx-roundtrip", tid).expect("register");

    assert!(registry::push_packet(tid, false, b"app-to-driver").is_ok());

    let mut out = [0u8; 64];
    let popped = registry::try_pop_packet(tid, true, &mut out).expect("driver pop must succeed");
    assert_eq!(popped, Some(b"app-to-driver".len()));
    assert_eq!(&out[..b"app-to-driver".len()], b"app-to-driver");

    registry::reset_for_test();
}

/// The driver pushes (RX ring); an app pops using the driver's tid and gets
/// the same bytes back.
#[test_case]
fn test_registry_push_pop_round_trip_rx_direction() {
    registry::reset_for_test();
    let tid = 222;
    registry::register(b"nic:rx-roundtrip", tid).expect("register");

    assert!(registry::push_packet(tid, true, b"driver-to-app").is_ok());

    let mut out = [0u8; 64];
    let popped = registry::try_pop_packet(tid, false, &mut out).expect("app pop must succeed");
    assert_eq!(popped, Some(b"driver-to-app".len()));
    assert_eq!(&out[..b"driver-to-app".len()], b"driver-to-app");

    registry::reset_for_test();
}

/// Pushing past `RING_CAPACITY` is rejected on the last push, and every
/// previously queued packet remains intact and poppable.
#[test_case]
fn test_registry_push_rejects_full_ring_and_preserves_existing() {
    registry::reset_for_test();
    let tid = 333;
    registry::register(b"nic:full-ring", tid).expect("register");

    for i in 0..registry::RING_CAPACITY {
        let payload = [i as u8; 4];
        assert!(
            registry::push_packet(tid, false, &payload).is_ok(),
            "push {} should succeed while under capacity",
            i
        );
    }

    // One more, past capacity, must fail.
    assert_eq!(
        registry::push_packet(tid, false, b"overflow"),
        Err(SyscallError::InvalidArg)
    );

    // Every one of the RING_CAPACITY packets must still pop out, in order,
    // untouched by the rejected push.
    let mut out = [0u8; 4];
    for i in 0..registry::RING_CAPACITY {
        let popped = registry::try_pop_packet(tid, true, &mut out).unwrap();
        assert_eq!(popped, Some(4));
        assert_eq!(
            out, [i as u8; 4],
            "packet {} must be untouched by the rejected push",
            i
        );
    }
    // Ring is now empty.
    assert_eq!(registry::try_pop_packet(tid, true, &mut out).unwrap(), None);

    registry::reset_for_test();
}

/// Packets pop in the exact order they were pushed (FIFO).
#[test_case]
fn test_registry_pop_fifo_order() {
    registry::reset_for_test();
    let tid = 444;
    registry::register(b"nic:fifo", tid).expect("register");

    assert!(registry::push_packet(tid, false, b"A").is_ok());
    assert!(registry::push_packet(tid, false, b"B").is_ok());
    assert!(registry::push_packet(tid, false, b"C").is_ok());

    let mut out = [0u8; 4];
    assert_eq!(
        registry::try_pop_packet(tid, true, &mut out).unwrap(),
        Some(1)
    );
    assert_eq!(&out[..1], b"A");
    assert_eq!(
        registry::try_pop_packet(tid, true, &mut out).unwrap(),
        Some(1)
    );
    assert_eq!(&out[..1], b"B");
    assert_eq!(
        registry::try_pop_packet(tid, true, &mut out).unwrap(),
        Some(1)
    );
    assert_eq!(&out[..1], b"C");
    assert_eq!(registry::try_pop_packet(tid, true, &mut out).unwrap(), None);

    registry::reset_for_test();
}

/// Pushing and popping past the `RING_CAPACITY` boundary multiple times
/// proves `head`/`tail` wrap correctly and never alias live data.
#[test_case]
fn test_registry_ring_wraparound() {
    registry::reset_for_test();
    let tid = 555;
    registry::register(b"nic:wraparound", tid).expect("register");

    // Push 25 two-byte packets (indices 0..24), leaving tail at 25.
    for i in 0u16..25 {
        assert!(registry::push_packet(tid, false, &i.to_le_bytes()).is_ok());
    }

    // Pop 20 of them (0..19), advancing head past the array's midpoint.
    let mut out = [0u8; 2];
    for i in 0u16..20 {
        let popped = registry::try_pop_packet(tid, true, &mut out).unwrap();
        assert_eq!(popped, Some(2));
        assert_eq!(u16::from_le_bytes(out), i);
    }

    // Push 15 more (indices 25..39). Tail wraps from 25 past RING_CAPACITY
    // (32) back around to 8; count is now 5 (20..24) + 15 (25..39) = 20.
    for i in 25u16..40 {
        assert!(registry::push_packet(tid, false, &i.to_le_bytes()).is_ok());
    }

    // Pop all 20 remaining packets; they must come out in exact order
    // (20..39), proving both head and tail wrapped consistently.
    for i in 20u16..40 {
        let popped = registry::try_pop_packet(tid, true, &mut out).unwrap();
        assert_eq!(popped, Some(2));
        assert_eq!(
            u16::from_le_bytes(out),
            i,
            "wraparound must preserve FIFO order"
        );
    }
    assert_eq!(registry::try_pop_packet(tid, true, &mut out).unwrap(), None);

    registry::reset_for_test();
}

/// Pushing/popping against an unregistered `driver_id` fails cleanly.
#[test_case]
fn test_registry_push_pop_unknown_driver_id_returns_invalid_arg() {
    registry::reset_for_test();
    assert_eq!(
        registry::push_packet(999_999, false, b"x"),
        Err(SyscallError::InvalidArg)
    );
    let mut out = [0u8; 4];
    assert_eq!(
        registry::try_pop_packet(999_999, false, &mut out),
        Err(SyscallError::InvalidArg)
    );
}

// ---------------------------------------------------------------------------
// syscall_net_send_impl / syscall_net_recv_impl tests (via dispatch_checked)
// ---------------------------------------------------------------------------

/// `NetSend` rejects a zero length and a length exceeding `MAX_PACKET_LEN`,
/// and rejects a canonical-but-unmapped packet pointer.
#[test_case]
fn test_net_send_syscall_validates_packet_len_and_ptr() {
    registry::reset_for_test();
    let task_id = spawn_driver_task();
    registry::register(b"nic:send-validate", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x1000, b"payload");

    let zero_len = dispatch_checked(SyscallId::NET_SEND, task_id as u64, user_va, 0, 0);
    assert_eq!(zero_len, Err(SyscallError::InvalidArg));

    let too_long = dispatch_checked(
        SyscallId::NET_SEND,
        task_id as u64,
        user_va,
        (registry::MAX_PACKET_LEN + 1) as u64,
        0,
    );
    assert_eq!(too_long, Err(SyscallError::InvalidArg));

    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFD_0000;
    let bad_ptr = dispatch_checked(SyscallId::NET_SEND, task_id as u64, unmapped_va, 8, 0);
    assert_eq!(bad_ptr, Err(SyscallError::InvalidArg));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `NetSend` against an unregistered `driver_id` fails.
#[test_case]
fn test_net_send_syscall_unknown_driver_id() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (user_va, phys) = write_bytes_to_user_page(0x2000, b"payload");
    let res = dispatch_checked(SyscallId::NET_SEND, 999_999, user_va, 7, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    release_user_page(user_va, phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// The role-based direction rule: an app's `NetSend`/`NetRecv` against a
/// driver's tid touches the opposite ring from that same driver task calling
/// `NetSend`/`NetRecv` on its own tid.
#[test_case]
fn test_net_send_recv_syscalls_role_based_direction() {
    registry::reset_for_test();
    let driver_task = spawn_driver_task();
    registry::register(b"nic:role-based", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let driver_slot = task_id_slot(driver_task);
    let app_slot = task_id_slot(app_task);

    // App sends -> lands in the TX ring -> only the driver's own-tid NetRecv sees it.
    set_running_slot_for_test(Some(app_slot));
    let (app_pkt_va, app_pkt_phys) = write_bytes_to_user_page(0x3000, b"from-app");
    let send_res = dispatch_checked(
        SyscallId::NET_SEND,
        driver_task as u64,
        app_pkt_va,
        b"from-app".len() as u64,
        0,
    );
    assert!(send_res.is_ok());

    set_running_slot_for_test(Some(driver_slot));
    let (out_va, out_phys) = write_bytes_to_user_page(0x4000, &[0u8; 32]);
    let recv_res = dispatch_checked(SyscallId::NET_RECV, driver_task as u64, out_va, 32, 0);
    assert_eq!(recv_res, Ok(b"from-app".len() as u64));

    // Driver sends (on its own tid) -> lands in the RX ring -> only an app's
    // NetRecv against the driver's tid sees it.
    let (driver_pkt_va, driver_pkt_phys) = write_bytes_to_user_page(0x5000, b"from-driver");
    let send_res2 = dispatch_checked(
        SyscallId::NET_SEND,
        driver_task as u64,
        driver_pkt_va,
        b"from-driver".len() as u64,
        0,
    );
    assert!(send_res2.is_ok());

    set_running_slot_for_test(Some(app_slot));
    let recv_res2 = dispatch_checked(SyscallId::NET_RECV, driver_task as u64, out_va, 32, 0);
    assert_eq!(recv_res2, Ok(b"from-driver".len() as u64));

    release_user_page(app_pkt_va, app_pkt_phys);
    release_user_page(driver_pkt_va, driver_pkt_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// `NetRecv` rejects an unmapped destination buffer.
#[test_case]
fn test_net_recv_syscall_validates_buf_ptr() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    registry::register(b"nic:recv-validate", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let unmapped_va = vmm::USER_HEAP_BASE + 0x0FFC_0000;
    let res = dispatch_checked(SyscallId::NET_RECV, task_id as u64, unmapped_va, 32, 0);
    assert_eq!(res, Err(SyscallError::InvalidArg));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `NetRecv` with `timeout_ms == 0` on an empty ring returns `Timeout`
/// immediately without entering the polling loop (this call must return
/// promptly, not hang the test).
#[test_case]
fn test_net_recv_syscall_timeout_zero_polls_once_without_blocking() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    registry::register(b"nic:zero-timeout", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (out_va, out_phys) = write_bytes_to_user_page(0x6000, &[0u8; 32]);
    let res = dispatch_checked(SyscallId::NET_RECV, task_id as u64, out_va, 32, 0);
    assert_eq!(res, Err(SyscallError::Timeout));

    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// `NetRecv` with a non-zero timeout and no producer gives up on its own
/// once the deadline elapses, mirroring `IrqWait`'s bounded-wait contract
/// (`test_irq_wait_times_out_when_no_irq_fires`).
#[test_case]
fn test_net_recv_syscall_timeout_elapses_when_no_producer() {
    registry::reset_for_test();
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    registry::register(b"nic:times-out", task_id).expect("register");
    let slot = task_id_slot(task_id);
    set_running_slot_for_test(Some(slot));

    let (out_va, out_phys) = write_bytes_to_user_page(0x7000, &[0u8; 32]);
    // No NetSend anywhere in this test: the ring never fills, so a bounded
    // NetRecv must give up on its own.
    let res = dispatch_checked(SyscallId::NET_RECV, task_id as u64, out_va, 32, 20);
    assert_eq!(res, Err(SyscallError::Timeout));

    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    registry::reset_for_test();
}

/// A packet larger than the caller's buffer is truncated, not rejected.
///
/// Uses two distinct tasks (an "app" sender and the driver receiver) rather
/// than one task sending and popping on its own tid: per the role-based
/// direction rule, a driver pushing on its own tid lands in its RX ring, but
/// that same driver popping on its own tid reads its TX ring — the two never
/// meet, by design (that duplex separation is the entire point of the rule).
#[test_case]
fn test_net_recv_syscall_truncates_to_buffer_length() {
    registry::reset_for_test();
    let driver_task = spawn_driver_task();
    registry::register(b"nic:truncate", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let app_slot = task_id_slot(app_task);
    let driver_slot = task_id_slot(driver_task);

    set_running_slot_for_test(Some(app_slot));
    let (pkt_va, pkt_phys) = write_bytes_to_user_page(0x8000, b"0123456789");
    let send_res = dispatch_checked(SyscallId::NET_SEND, driver_task as u64, pkt_va, 10, 0);
    assert!(send_res.is_ok());

    set_running_slot_for_test(Some(driver_slot));
    let (out_va, out_phys) = write_bytes_to_user_page(0x9000, &[0u8; 4]);
    let recv_res = dispatch_checked(SyscallId::NET_RECV, driver_task as u64, out_va, 4, 0);
    assert_eq!(
        recv_res,
        Ok(4),
        "oversized packet must be truncated to the buffer length"
    );

    release_user_page(pkt_va, pkt_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// If the owning driver task exits mid-flight, `NetSend`/`NetRecv` against
/// its now-stale `driver_id` fail instead of touching freed memory.
#[test_case]
fn test_net_send_recv_cleanup_on_exit() {
    registry::reset_for_test();
    let driver_task = spawn_driver_task();
    registry::register(b"nic:cleanup-on-exit", driver_task).expect("register");
    let app_task = sched::spawn_kernel_task(test_task_loop).expect("spawn app task");
    let app_slot = task_id_slot(app_task);

    set_running_slot_for_test(Some(app_slot));
    let (pkt_va, pkt_phys) = write_bytes_to_user_page(0xA000, b"still-alive");
    let send_res = dispatch_checked(
        SyscallId::NET_SEND,
        driver_task as u64,
        pkt_va,
        b"still-alive".len() as u64,
        0,
    );
    assert!(send_res.is_ok(), "driver is alive; NetSend must succeed");

    // The driver task exits -- remove_task must release its DriverRegistry
    // entry (Step 1), taking its packet rings down with it.
    set_running_slot_for_test(None);
    sched::terminate_task(driver_task);

    set_running_slot_for_test(Some(app_slot));
    let send_after_exit = dispatch_checked(
        SyscallId::NET_SEND,
        driver_task as u64,
        pkt_va,
        b"still-alive".len() as u64,
        0,
    );
    assert_eq!(
        send_after_exit,
        Err(SyscallError::InvalidArg),
        "NetSend against an exited driver's tid must fail, not touch freed memory"
    );

    let (out_va, out_phys) = write_bytes_to_user_page(0xB000, &[0u8; 32]);
    let recv_after_exit = dispatch_checked(SyscallId::NET_RECV, driver_task as u64, out_va, 32, 0);
    assert_eq!(
        recv_after_exit,
        Err(SyscallError::InvalidArg),
        "NetRecv against an exited driver's tid must fail, not touch freed memory"
    );

    release_user_page(pkt_va, pkt_phys);
    release_user_page(out_va, out_phys);
    set_running_slot_for_test(None);
    sched::terminate_task(app_task);
    registry::reset_for_test();
}

/// `syscall_name_for_number` resolves both new syscall numbers.
#[test_case]
fn test_net_syscall_debug_names() {
    assert_eq!(syscall_name_for_number(SyscallId::NET_SEND), "NetSend");
    assert_eq!(syscall_name_for_number(SyscallId::NET_RECV), "NetRecv");
}
