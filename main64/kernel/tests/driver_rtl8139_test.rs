//! Integration tests for the RTL8139 user-space driver lifecycle and abstractions.

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
use kaos_kernel::syscall::{dispatch_checked, SyscallId, SYSCALL_OK};

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

/// Tests that a simulated RTL8139 driver task with MMIO and IRQ capabilities
/// can map its device registers and subscribe to its interrupt line.
#[test_case]
fn test_rtl8139_driver_grants_and_mapping() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Simulated RTL8139 BAR physical 0xFEB0_0000, 256 bytes, IRQ line 11
    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::MMIO | Capabilities::IRQ,
        grants,
    )));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Map RTL8139 MMIO BAR
    let va = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 256, 0, 0)
        .expect("RTL8139 MMIO mapping must succeed");
    assert_eq!(
        va,
        vmm::USER_MMIO_BASE,
        "MMIO mapping starts at USER_MMIO_BASE"
    );

    // Subscribe to RTL8139 IRQ
    let irq_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        irq_res,
        Ok(SYSCALL_OK),
        "RTL8139 IRQ subscription must succeed"
    );

    // Clean up
    let unmap_res = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va, 256, 0, 0);
    assert_eq!(unmap_res, Ok(SYSCALL_OK), "Unmap MMIO must succeed");

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    kaos_kernel::drivers::irq_bridge::reset_bindings_for_test();
}

/// Tests that a driver task with MMIO capabilities can allocate contiguous DMA buffers,
/// translate their virtual addresses to physical frames, and free them cleanly.
#[test_case]
fn test_rtl8139_driver_dma_allocation_and_translation() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::MMIO | Capabilities::IRQ,
        grants,
    )));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Step 1: Map a user page for the out_phys pointer parameter.
    let user_out_va = vmm::USER_HEAP_BASE + 0x4000;
    let out_phys_frame = vmm::page_table::alloc_frame_phys().unwrap();
    let out_pfn = vmm::page_table::phys_to_pfn(out_phys_frame);
    vmm::map_user_page(user_out_va, out_pfn, true).unwrap();

    // Step 2: Allocate a 4-page (16 KiB) contiguous DMA buffer.
    let va = dispatch_checked(SyscallId::ALLOC_DMA, 4, user_out_va, 0, 0)
        .expect("AllocDma must succeed");
    assert!(va >= vmm::USER_MMIO_BASE);

    // SAFETY: user_out_va is mapped and initialized by AllocDma.
    let out_phys = unsafe { core::ptr::read(user_out_va as *const u64) };
    assert_ne!(
        out_phys, 0,
        "AllocDma must return non-zero physical address"
    );

    // Step 3: Translate virtual address back to physical address.
    let translated_phys =
        dispatch_checked(SyscallId::VIRT_TO_PHYS, va, 0, 0, 0).expect("VirtToPhys must succeed");
    assert_eq!(
        translated_phys, out_phys,
        "VirtToPhys must match physical address returned by AllocDma"
    );

    // Step 4: Free the DMA buffer.
    let free_res = dispatch_checked(SyscallId::FREE_DMA, va, 4, 0, 0);
    assert_eq!(free_res, Ok(SYSCALL_OK), "FreeDma must succeed");

    vmm::unmap_without_release(user_out_va);
    pmm::with_pmm(|mgr| mgr.release_pfn(out_pfn));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}
