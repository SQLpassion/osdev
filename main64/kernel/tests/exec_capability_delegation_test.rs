//! Integration tests for `Exec`'s capability-delegation feature (issue #101).
//!
//! Kept in its own boot session, separate from `process_contract_test.rs`:
//! terminating a real, never-scheduled exec'd child releases its
//! loader-owned code frame back to the PMM immediately, while the
//! containing address space is only torn down later, lazily, on an actual
//! context switch these synchronous tests never trigger. A later test in the
//! same boot session that also touches the user-code region can observe the
//! resulting inconsistent page-table state — a hazard of address-space
//! teardown under this synchronous test harness (not a production bug: real
//! tasks run under genuine CR3 switches), and orthogonal to what this file is
//! testing. Keeping the one end-to-end `Exec` test that spawns a real child
//! as the only such test in this boot session sidesteps it entirely.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::capabilities::Capabilities;
use kaos_kernel::scheduler;
use kaos_kernel::syscall::{self, SyscallId};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);
    scheduler::init();

    kaos_kernel::drivers::ata::init();
    kaos_kernel::drivers::block::init_ata();
    let vol = kaos_kernel::io::fat32::Fat32Volume::mount(0)
        .expect("FAT32 superfloppy must mount at LBA 0 in the test image");
    kaos_kernel::io::vfs::mount(alloc::boxed::Box::new(
        kaos_kernel::io::fat32::Fat32Fs::new(vol),
    ));

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: a privileged caller (e.g. the boot shell) delegating capabilities
/// via `Exec` gets exactly the capabilities it requested attached to the
/// child's `DriverCaps` block, while the child itself remains unprivileged.
#[test_case]
fn test_exec_privileged_caller_delegates_all_requested_capabilities() {
    scheduler::set_kernel_address_space_cr3(vmm::get_pml4_address());

    // A privileged caller task. Uses a plain kernel task plus
    // `set_task_privileged_for_test` rather than `spawn_user_task(..,
    // privileged: true)` with a cloned CR3: this test never runs the
    // caller's own code, and tearing down a cloned-but-never-scheduled user
    // address space is its own separate hazard unrelated to what this test
    // is verifying (capability delegation, not address-space teardown).
    extern "C" fn caller_loop() -> ! {
        loop {
            scheduler::yield_now();
        }
    }
    let caller_id = scheduler::spawn_kernel_task(caller_loop).expect("spawn caller task");
    assert!(
        scheduler::set_task_privileged_for_test(caller_id, true),
        "should mark caller task privileged"
    );
    let caller_slot = scheduler::task_id_slot(caller_id);

    const NAME_VA: u64 = vmm::USER_CODE_BASE + 0x80_000;
    let phys = vmm::page_table::alloc_frame_phys().expect("frame alloc for name page");
    let pfn = vmm::page_table::phys_to_pfn(phys);
    vmm::map_user_page(NAME_VA, pfn, true).expect("map name page");
    // SAFETY: `NAME_VA` was just mapped present, writable, and user-accessible.
    unsafe {
        core::ptr::copy_nonoverlapping(c"HELLO.BIN".as_ptr() as *const u8, NAME_VA as *mut u8, 10);
    }

    scheduler::set_running_slot_for_test(Some(caller_slot));
    let requested = (Capabilities::SPAWN_DRIVER | Capabilities::UNLOAD_DRIVER).bits() as u64;
    let tid = syscall::dispatch_checked(SyscallId::EXEC, NAME_VA, requested, 0, 0)
        .expect("Exec must succeed for a privileged caller requesting delegation")
        as usize;
    scheduler::set_running_slot_for_test(None);

    assert!(
        !scheduler::is_task_privileged(tid),
        "a child spawned via Exec must never itself become privileged, \
         regardless of delegated capabilities"
    );

    scheduler::set_running_slot_for_test(Some(scheduler::task_id_slot(tid)));
    let attached = scheduler::current_task_caps()
        .expect("DriverCaps must be attached when capabilities were delegated");
    assert_eq!(
        attached.flags.bits(),
        requested as u32,
        "child must receive exactly the requested capabilities, nothing more or less"
    );
    scheduler::set_running_slot_for_test(None);

    scheduler::terminate_task(tid);
    scheduler::terminate_task(caller_id);
}

/// Contract: a privileged caller may delegate anything it requests, even
/// capabilities it does not itself hold — privilege overrides the
/// caller-capability intersection entirely.
#[test_case]
fn test_resolve_delegated_capabilities_privileged_caller_gets_everything_requested() {
    let requested = Capabilities::SPAWN_DRIVER | Capabilities::UNLOAD_DRIVER;
    let granted = syscall::resolve_delegated_capabilities(true, Capabilities::NONE, requested);
    assert_eq!(
        granted, requested,
        "a privileged caller must be able to delegate capabilities it does not hold itself"
    );
}

/// Contract: an unprivileged caller with no capabilities of its own cannot
/// delegate anything, no matter what it requests.
#[test_case]
fn test_resolve_delegated_capabilities_unprivileged_caller_without_caps_grants_none() {
    let requested = Capabilities::SPAWN_DRIVER | Capabilities::UNLOAD_DRIVER;
    let granted = syscall::resolve_delegated_capabilities(false, Capabilities::NONE, requested);
    assert_eq!(
        granted,
        Capabilities::NONE,
        "an unprivileged caller with no capabilities of its own must not be able \
         to grant any to its child"
    );
}

/// Contract: an unprivileged caller can delegate at most the capabilities it
/// already holds itself — requesting more than it has narrows the grant to
/// the intersection, never handing the child extra bits.
#[test_case]
fn test_resolve_delegated_capabilities_unprivileged_caller_delegates_only_its_own() {
    let caller_flags = Capabilities::SPAWN_DRIVER;
    let requested = Capabilities::SPAWN_DRIVER | Capabilities::UNLOAD_DRIVER;
    let granted = syscall::resolve_delegated_capabilities(false, caller_flags, requested);
    assert_eq!(
        granted,
        Capabilities::SPAWN_DRIVER,
        "child must receive only the capabilities the caller itself held, \
         never UNLOAD_DRIVER which the caller lacked"
    );
}

/// Contract: requesting zero capabilities always yields zero, regardless of
/// the caller's own privilege or capability state — the pre-existing
/// `exec()` wrapper's default behavior stays a strict no-op for delegation.
#[test_case]
fn test_resolve_delegated_capabilities_zero_requested_always_yields_none() {
    let all_caps = Capabilities::SPAWN_DRIVER | Capabilities::UNLOAD_DRIVER;

    assert_eq!(
        syscall::resolve_delegated_capabilities(true, Capabilities::NONE, Capabilities::NONE),
        Capabilities::NONE,
        "privileged caller requesting nothing must still be granted nothing"
    );
    assert_eq!(
        syscall::resolve_delegated_capabilities(false, all_caps, Capabilities::NONE),
        Capabilities::NONE,
        "unprivileged caller requesting nothing must still be granted nothing, \
         even if it holds every capability itself"
    );
}
