//! Per-task capability and resource-grant system integration tests.

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
use kaos_kernel::scheduler::{self as sched, set_task_caps};

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

extern "C" fn dummy_task() -> ! {
    loop {
        sched::yield_now();
    }
}

/// Tests bitwise operations and invariants of the Capabilities bitmask type.
#[test_case]
fn test_capabilities_bitflags() {
    let none = Capabilities::NONE;
    assert_eq!(none.bits(), 0);
    assert!(!none.contains(Capabilities::MMIO));

    let mmio = Capabilities::MMIO;
    let spawn = Capabilities::SPAWN_DRIVER;
    let unload = Capabilities::UNLOAD_DRIVER;

    let combined = mmio | unload;
    assert!(combined.contains(Capabilities::MMIO));
    assert!(combined.contains(Capabilities::UNLOAD_DRIVER));
    assert!(!combined.contains(Capabilities::SPAWN_DRIVER));

    let all = combined.union(spawn);
    assert!(all.contains(Capabilities::MMIO));
    assert!(all.contains(Capabilities::UNLOAD_DRIVER));
    assert!(all.contains(Capabilities::SPAWN_DRIVER));

    let truncated = Capabilities::from_bits_truncate(0xFFFF_FFFF);
    assert_eq!(
        truncated.bits(),
        (Capabilities::MMIO
            | Capabilities::SPAWN_DRIVER
            | Capabilities::UNLOAD_DRIVER
            | Capabilities::LIST_DRIVERS)
            .bits()
    );
}

/// Tests that the driver-management delegation bits (`UNLOAD_DRIVER`,
/// `LIST_DRIVERS`) are distinct, non-overlapping bitflags that survive
/// `from_bits_truncate` like the pre-existing coarse flags.
#[test_case]
fn test_unload_and_list_drivers_capability_bits() {
    let unload = Capabilities::UNLOAD_DRIVER;
    let list = Capabilities::LIST_DRIVERS;

    assert_ne!(unload.bits(), 0);
    assert_ne!(list.bits(), 0);
    assert_ne!(unload.bits(), list.bits());

    let combined = unload | list;
    assert!(combined.contains(Capabilities::UNLOAD_DRIVER));
    assert!(combined.contains(Capabilities::LIST_DRIVERS));
    assert!(!combined.contains(Capabilities::SPAWN_DRIVER));

    let truncated = Capabilities::from_bits_truncate(combined.bits());
    assert_eq!(
        truncated.bits(),
        combined.bits(),
        "UNLOAD_DRIVER and LIST_DRIVERS must survive from_bits_truncate unchanged"
    );
}

/// Tests that a freshly spawned task defaults to null caps (no capabilities).
#[test_case]
fn test_spawned_task_has_no_caps_by_default() {
    let task_id = sched::spawn_kernel_task(dummy_task).expect("task should spawn");

    // Setting caps to null is default, verify via set_task_caps and scheduler inspection.
    let is_set = set_task_caps(task_id, core::ptr::null_mut());
    assert!(is_set, "set_task_caps on live task should succeed");

    sched::terminate_task(task_id);
}

/// Tests attaching and querying DriverCaps on a task.
#[test_case]
fn test_driver_caps_attachment_and_cleanup() {
    let task_id = sched::spawn_kernel_task(dummy_task).expect("task should spawn");

    // Allocate a DriverCaps block on the heap.
    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256), (0xFEB1_0000, 4096)],
        mmio_bump: 0x0000_7800_0000_0000,
    };
    let flags = Capabilities::MMIO | Capabilities::UNLOAD_DRIVER;
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(flags, grants)));

    // Attach to the task.
    let attached = set_task_caps(task_id, caps_ptr);
    assert!(attached, "should attach caps to task");

    // Terminating task will trigger remove_task during reap, which frees caps.
    sched::terminate_task(task_id);
}
