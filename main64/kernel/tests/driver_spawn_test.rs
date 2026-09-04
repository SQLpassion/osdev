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
use kaos_kernel::drivers::driver_db::{self, BindError};
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

/// Tests that a spawned driver can never inherit SPAWN_DRIVER, and that unknown
/// capability bits are dropped rather than stored.
#[test_case]
fn test_sanitize_driver_caps_strips_spawn_driver() {
    let all_bits = driver_db::sanitize_driver_caps(u64::MAX);
    assert!(
        !all_bits.contains(Capabilities::SPAWN_DRIVER),
        "SPAWN_DRIVER must never be granted to a spawned driver"
    );
    assert_eq!(
        all_bits,
        driver_db::DRIVER_GRANTABLE_CAPS,
        "an all-bits request must be clamped to exactly the driver-grantable set"
    );

    let requested = Capabilities::MMIO | Capabilities::IRQ | Capabilities::SPAWN_DRIVER;
    let granted = driver_db::sanitize_driver_caps(requested.bits() as u64);
    assert!(
        granted.contains(Capabilities::MMIO) && granted.contains(Capabilities::IRQ),
        "MMIO and IRQ must survive sanitization"
    );
    assert!(
        !granted.contains(Capabilities::SPAWN_DRIVER),
        "SPAWN_DRIVER must be masked off even when explicitly requested"
    );

    assert_eq!(
        driver_db::sanitize_driver_caps(0),
        Capabilities::NONE,
        "an empty request must stay empty"
    );
}

/// Tests that the driver database resolves registered binaries case-insensitively
/// (FAT32 8.3 short names) and rejects unregistered ones.
#[test_case]
fn test_driver_db_lookup() {
    assert!(
        driver_db::is_known_driver("rtl8139.bin"),
        "rtl8139.bin must be a registered driver"
    );
    assert!(
        driver_db::is_known_driver("RTL8139.BIN"),
        "driver names must be matched case-insensitively"
    );
    assert!(
        driver_db::is_known_driver("INTLNIC.BIN"),
        "intlnic.bin must be a registered driver"
    );
    assert!(
        !driver_db::is_known_driver("hello.bin"),
        "an ordinary user program must not be a registered driver"
    );

    let rtl_ids = driver_db::lookup_driver("rtl8139.bin").expect("rtl8139 entry");
    assert!(
        rtl_ids.contains(&(0x10EC, 0x8139)),
        "rtl8139.bin must be bound to 10EC:8139"
    );
    assert!(
        !rtl_ids.contains(&(0x8086, 0x100E)),
        "rtl8139.bin must not be allowed to bind to an Intel NIC"
    );
}

/// Tests that grant derivation refuses to produce grants for an unregistered binary,
/// which is what keeps an arbitrary program from being handed device resources.
#[test_case]
fn test_derive_grants_rejects_unknown_driver() {
    assert_eq!(
        driver_db::derive_grants("hello.bin").err(),
        Some(BindError::UnknownDriver),
        "an unregistered binary must not receive any derived grant"
    );
}

/// Tests that a caller's grant request is accepted only when it falls inside the
/// kernel-derived grant — the check that turns a forged request into PermissionDenied
/// instead of an arbitrary physical mapping.
#[test_case]
fn test_request_must_match_derived_grants() {
    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEBC_0000, 0x2_0000)],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };

    assert!(
        driver_db::request_matches_grants(&grants, 0xFEBC_0000, 11),
        "a request naming the device's own BAR base and IRQ must be accepted"
    );
    assert!(
        driver_db::request_matches_grants(&grants, 0xFEBC_1000, 0xFF),
        "an address inside the granted window with no IRQ preference must be accepted"
    );
    assert!(
        driver_db::request_matches_grants(&grants, 0, 0xFF),
        "an empty request must be accepted"
    );

    // Kernel physical memory: the exact escalation this check exists to block.
    assert!(
        !driver_db::request_matches_grants(&grants, 0x0010_0000, 11),
        "a request for physical memory outside the device BAR must be rejected"
    );
    assert!(
        !driver_db::request_matches_grants(&grants, 0xFEBE_0000, 11),
        "a request one byte past the granted window must be rejected"
    );
    assert!(
        !driver_db::request_matches_grants(&grants, 0xFEBC_0000, 10),
        "a request for an IRQ the device does not own must be rejected"
    );
}
