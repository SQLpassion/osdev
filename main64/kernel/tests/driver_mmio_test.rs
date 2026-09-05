//! Integration tests for MMIO mapping syscalls (MapPhysical / UnmapPhysical).

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

/// Tests that MapPhysical fails with PermissionDenied if the task has no capabilities.
#[test_case]
fn test_map_physical_no_caps_fails() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    set_running_slot_for_test(Some(task_id_slot(task_id)));

    let res = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 256, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "MapPhysical without DriverCaps must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that MapPhysical fails when requesting a physical region not granted in ResourceGrants.
#[test_case]
fn test_map_physical_unauthorized_grant_fails() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Grant region 0xFEB0_0000..0xFEB0_0100
    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Requesting a physical address outside the grant
    let res = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB1_0000, 256, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "MapPhysical for unauthorized physical range must return PermissionDenied"
    );

    // Requesting a physical address overlapping but extending beyond the grant
    let res_overflow = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 512, 0, 0);
    assert_eq!(
        res_overflow,
        Err(SyscallError::PermissionDenied),
        "MapPhysical exceeding grant size must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that MapPhysical fails on invalid arguments (e.g. length = 0 or u64 overflow).
#[test_case]
fn test_map_physical_invalid_arg_fails() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // len = 0
    let res_zero = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 0, 0, 0);
    assert_eq!(
        res_zero,
        Err(SyscallError::InvalidArg),
        "MapPhysical with len=0 must return InvalidArg"
    );

    // Overflow
    let res_overflow = dispatch_checked(SyscallId::MAP_PHYSICAL, u64::MAX - 10, 20, 0, 0);
    assert_eq!(
        res_overflow,
        Err(SyscallError::InvalidArg),
        "MapPhysical with overflowing range must return InvalidArg"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests successful mapping, bump pointer advance, and unmapping.
#[test_case]
fn test_map_and_unmap_physical_lifecycle() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 0x1000), (0xFEB1_0000, 0x2000)],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // First map: 0xFEB0_0000, len 256
    let va1 = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 256, 0, 0)
        .expect("first map must succeed");
    assert_eq!(
        va1,
        vmm::USER_MMIO_BASE,
        "first mapping must start at USER_MMIO_BASE"
    );

    // Second map: 0xFEB1_0000, len 4096 -> should advance by 4096 (1 page)
    let va2 = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB1_0000, 4096, 0, 0)
        .expect("second map must succeed");
    assert_eq!(
        va2,
        vmm::USER_MMIO_BASE + 4096,
        "second mapping must advance bump pointer past the first page"
    );

    // Unmap the first region
    let unmap_res = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va1, 256, 0, 0);
    assert_eq!(unmap_res, Ok(SYSCALL_OK), "unmapping va1 must succeed");

    // Unmap the second region
    let unmap_res2 = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va2, 4096, 0, 0);
    assert_eq!(unmap_res2, Ok(SYSCALL_OK), "unmapping va2 must succeed");

    // Unmap invalid VA outside MMIO window
    let unmap_invalid = dispatch_checked(SyscallId::UNMAP_PHYSICAL, 0x0000_1000, 256, 0, 0);
    assert_eq!(
        unmap_invalid,
        Err(SyscallError::InvalidArg),
        "unmapping non-MMIO VA must return InvalidArg"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that AllocDma requires MMIO capability specifically — IRQ alone is
/// not a substitute. `syscall_alloc_dma_impl`'s doc comment used to claim
/// "MMIO or IRQ" while the code only ever checked MMIO; a driver holding only
/// IRQ has no way to program a DMA buffer's physical address into any device
/// register, so MMIO is the correct (and only) gate.
#[test_case]
fn test_alloc_dma_requires_mmio_not_irq_alone() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let res = dispatch_checked(SyscallId::ALLOC_DMA, 1, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "AllocDma with only IRQ capability (no MMIO) must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that FreeDma requires MMIO capability specifically, mirroring AllocDma.
#[test_case]
fn test_free_dma_requires_mmio_not_irq_alone() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::IRQ, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let res = dispatch_checked(SyscallId::FREE_DMA, vmm::USER_MMIO_BASE, 1, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "FreeDma with only IRQ capability (no MMIO) must return PermissionDenied"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that MMIO capability alone (no IRQ) is sufficient for a full
/// AllocDma/FreeDma lifecycle — confirming IRQ is not required, only MMIO.
#[test_case]
fn test_alloc_and_free_dma_succeeds_with_mmio_capability_alone() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let va = dispatch_checked(SyscallId::ALLOC_DMA, 1, 0, 0, 0)
        .expect("AllocDma with MMIO capability alone must succeed");
    assert_eq!(
        va,
        vmm::USER_MMIO_BASE,
        "the DMA buffer must be mapped at the start of the MMIO VA window"
    );

    let free_res = dispatch_checked(SyscallId::FREE_DMA, va, 1, 0, 0);
    assert_eq!(
        free_res,
        Ok(SYSCALL_OK),
        "FreeDma with MMIO capability alone must succeed"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that FreeDma refuses a VA that actually came from MapPhysical.
///
/// Before this fix, FreeDma and UnmapPhysical shared only a generic
/// "VA lies inside the MMIO window" check with no record of which allocator
/// produced a given range. Handing a MapPhysical VA to FreeDma would unmap
/// the device's register window and then call `pmm::release_pfn` on its
/// physical BAR address — silent corruption of a live device mapping instead
/// of a rejected call.
#[test_case]
fn test_free_dma_rejects_map_physical_va() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 0x1000)],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let va = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 0x1000, 0, 0)
        .expect("MapPhysical must succeed");

    let free_res = dispatch_checked(SyscallId::FREE_DMA, va, 1, 0, 0);
    assert_eq!(
        free_res,
        Err(SyscallError::InvalidArg),
        "FreeDma on a MapPhysical VA must be rejected, not silently release the BAR window"
    );

    // The rejected FreeDma attempt must not have consumed or corrupted the
    // MapPhysical allocation record: a real UnmapPhysical must still work.
    let unmap_res = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va, 0x1000, 0, 0);
    assert_eq!(
        unmap_res,
        Ok(SYSCALL_OK),
        "the MapPhysical mapping must still be intact and unmappable after the rejected FreeDma"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

/// Tests that UnmapPhysical refuses a VA that actually came from AllocDma.
///
/// Before this fix, UnmapPhysical on an AllocDma VA would unmap the pages
/// without ever releasing their physical frames to the PMM, leaking them
/// permanently instead of being rejected.
#[test_case]
fn test_unmap_physical_rejects_alloc_dma_va() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(Capabilities::MMIO, grants)));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    let va = dispatch_checked(SyscallId::ALLOC_DMA, 1, 0, 0, 0).expect("AllocDma must succeed");

    let unmap_res = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va, 4096, 0, 0);
    assert_eq!(
        unmap_res,
        Err(SyscallError::InvalidArg),
        "UnmapPhysical on an AllocDma VA must be rejected, not silently leak its RAM frames"
    );

    // The rejected UnmapPhysical attempt must not have consumed or corrupted
    // the AllocDma allocation record: a real FreeDma must still work.
    let free_res = dispatch_checked(SyscallId::FREE_DMA, va, 1, 0, 0);
    assert_eq!(
        free_res,
        Ok(SYSCALL_OK),
        "the AllocDma buffer must still be intact and freeable after the rejected UnmapPhysical"
    );

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}
