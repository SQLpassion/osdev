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
use kaos_kernel::drivers::pci::{self, BarType, PciBar, PciDevice};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
use kaos_kernel::scheduler::{
    self as sched, set_running_slot_for_test, set_task_caps, task_id_slot,
};
use kaos_kernel::syscall::{dispatch_checked, SyscallError, SyscallId, UserDriverGrants};

/// Builds a synthetic PCI device for tests that exercise the device-binding
/// registry directly. The QEMU configuration used by the test runner attaches
/// no PCI NIC, so `derive_grants` itself can never reach a live device here —
/// see `driver_db::reserve_device_for_test`.
fn fake_pci_device(bus: u8, device: u8, function: u8) -> PciDevice {
    PciDevice {
        bus,
        device,
        function,
        vendor_id: 0x10EC,
        device_id: 0x8139,
        class_code: 0x02,
        subclass: 0x00,
        prog_if: 0x00,
        revision_id: 0x00,
        header_type: 0x00,
        interrupt_line: 11,
        interrupt_pin: 1,
        bars: [PciBar {
            bar_type: BarType::Memory32 {
                address: 0xFEB0_0000,
                size: 256,
                prefetchable: false,
            },
            raw_value: 0,
        }; 6],
    }
}

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);
    sched::init();

    // Mounted so `test_spawn_driver_end_to_end_creates_ready_task_with_caps_and_parent`
    // can drive the real `SpawnDriver` syscall path (which loads its target
    // binary through the VFS) instead of stopping at an early rejection.
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

/// Tests that SpawnDriver fails closed — not open — when there is no current
/// task to authorize (`scheduler::current_task_id()` returns `None`).
///
/// Before this fix, the `None`-current-task fallback defaulted to `true`
/// (authorized), silently bypassing the capability gate for any caller this
/// check could not identify. An unreachable path today, but a landmine for
/// future code that might dispatch this syscall outside a task context.
#[test_case]
fn test_spawn_driver_with_no_current_task_fails_closed() {
    set_running_slot_for_test(None);

    let res = dispatch_checked(SyscallId::SPAWN_DRIVER, 0, 0, 0, 0);
    assert_eq!(
        res,
        Err(SyscallError::PermissionDenied),
        "SpawnDriver must fail closed, not open, when no task is currently running"
    );
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

/// Tests the full `SpawnDriver` success path end to end: spawning a binary
/// the kernel does not recognize as a *driver* (`derive_grants` returns
/// `BindError::UnknownDriver`, a deliberate "spawn as an ordinary Ring-3
/// program" fallback, not a rejection) still runs the complete
/// `syscall_spawn_driver_impl` sequence — parent linkage, the (empty, since
/// there is no bound device) grant, `DriverCaps` attachment, and finally
/// unblocking the task.
///
/// This is the only test in this file that reaches that far: every other
/// `SpawnDriver` test here stops at an early rejection, because the QEMU
/// configuration used by the test runner attaches no PCI NIC (see
/// `fake_pci_device`'s doc comment) — so this is what actually exercises the
/// task ending up `TaskState::Ready` (not left `Blocked` forever) and the
/// manually-allocated `DriverCaps` block (see `syscall_spawn_driver_impl`'s
/// step 9) landing on the right task.
#[test_case]
fn test_spawn_driver_end_to_end_creates_ready_task_with_caps_and_parent() {
    let caller_id = sched::spawn_kernel_task(test_task_loop).expect("spawn caller task");
    let caller_slot = task_id_slot(caller_id);

    let caller_grants = ResourceGrants {
        mmio_regions: vec![],
        irqs: vec![],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caller_caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::SPAWN_DRIVER,
        caller_grants,
    )));
    assert!(
        set_task_caps(caller_id, caller_caps_ptr),
        "caps should attach to the caller task"
    );

    // `read_user_string` (which resolves `name_ptr`) walks the currently
    // active page tables directly, so a plain mapped-and-written page is
    // enough here — no dedicated per-task address space is needed for a
    // kernel-mode caller task in this test harness.
    const NAME_VA: u64 = vmm::USER_CODE_BASE + 0x70_000;
    let name_phys = vmm::page_table::alloc_frame_phys().expect("frame alloc for name page");
    let name_pfn = vmm::page_table::phys_to_pfn(name_phys);
    vmm::map_user_page(NAME_VA, name_pfn, true).expect("map name page");
    // SAFETY: `NAME_VA` was just mapped present, writable, and user-accessible.
    unsafe {
        core::ptr::copy_nonoverlapping(c"HELLO.BIN".as_ptr() as *const u8, NAME_VA as *mut u8, 10);
    }

    set_running_slot_for_test(Some(caller_slot));
    let requested_caps = (Capabilities::MMIO | Capabilities::IRQ).bits() as u64;
    let tid = dispatch_checked(SyscallId::SPAWN_DRIVER, NAME_VA, requested_caps, 0, 0)
        .expect("SpawnDriver on an ordinary (non-driver) binary must still succeed")
        as usize;
    set_running_slot_for_test(None);

    assert_eq!(
        sched::task_state(tid),
        Some(sched::TaskState::Ready),
        "SpawnDriver must leave the new task Ready (unblocked) once setup completes, \
         not stuck Blocked forever"
    );
    assert!(
        sched::is_parent_of(caller_id, tid),
        "SpawnDriver must record the caller as the new task's parent"
    );

    set_running_slot_for_test(Some(task_id_slot(tid)));
    let attached_caps =
        sched::current_task_caps().expect("DriverCaps must be attached to the new task");
    assert!(
        attached_caps
            .flags
            .contains(Capabilities::MMIO | Capabilities::IRQ),
        "requested driver-grantable capabilities must reach the new task's DriverCaps"
    );
    set_running_slot_for_test(None);

    sched::terminate_task(tid);
    sched::terminate_task(caller_id);
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

/// Tests that a BAR whose page-aligned window would overflow `u64` is
/// skipped instead of silently wrapping into a corrupted grant.
///
/// `mmio_windows` used raw (non-checked) `u64` arithmetic to compute a BAR's
/// page-aligned end address, unlike every other address computation in
/// `driver_db`, which uses `checked_add`. A `Memory64` BAR's (address, size)
/// pair comes straight from PCI config-space registers a device (or a
/// misbehaving VM) controls; values close to `u64::MAX` would wrap the
/// page-aligned end low, and the subsequent `page_end - page_base`
/// subtraction would then underflow into a huge bogus length —
/// `syscall_map_physical_impl` trusts this window as an authoritative grant
/// without re-validating its internal consistency.
#[test_case]
fn test_mmio_windows_rejects_overflowing_bar() {
    let mut device = fake_pci_device(0, 8, 0);
    device.bars[0] = PciBar {
        bar_type: BarType::Memory64 {
            address: u64::MAX - 100,
            size: 200,
            prefetchable: false,
        },
        raw_value: 0,
    };
    for bar in device.bars.iter_mut().skip(1) {
        *bar = PciBar {
            bar_type: BarType::None,
            raw_value: 0,
        };
    }

    let windows = driver_db::mmio_windows_for_test(&device);
    assert!(
        windows.is_empty(),
        "a BAR whose page-aligned window overflows u64 must be skipped, \
         not silently wrapped into a corrupted grant"
    );
}

/// Tests that an ordinary, non-overflowing `Memory64` BAR still derives its
/// window correctly — the overflow guard in `mmio_windows` must not reject
/// legitimate BARs near (but not at) the top of the address space.
#[test_case]
fn test_mmio_windows_accepts_non_overflowing_bar() {
    let mut device = fake_pci_device(0, 9, 0);
    device.bars[0] = PciBar {
        bar_type: BarType::Memory64 {
            address: 0xFEBC_0000,
            size: 0x1234,
            prefetchable: false,
        },
        raw_value: 0,
    };
    for bar in device.bars.iter_mut().skip(1) {
        *bar = PciBar {
            bar_type: BarType::None,
            raw_value: 0,
        };
    }

    let windows = driver_db::mmio_windows_for_test(&device);
    assert_eq!(
        windows,
        vec![(0xFEBC_0000, 0x2000)],
        "a normal BAR must still be rounded out to whole 4 KiB pages"
    );
}

/// Tests that a second SpawnDriver-style claim on the same PCI device is
/// rejected while the first owning task is still alive, and succeeds again
/// once that task has terminated — the double-grant prevention this module
/// exists for.
///
/// This drives `driver_db`'s reservation registry directly (via
/// `reserve_device_for_test`/`confirm_binding`/`release_task`) rather than
/// through the full `SpawnDriver` syscall, because the QEMU configuration
/// used by the test runner attaches no PCI NIC: `derive_grants("rtl8139.bin")`
/// would only ever observe `BindError::DeviceNotPresent` here, never reaching
/// a live device to bind. The registry logic under test is identical either way
/// — `derive_grants` calls exactly this same `reserve_device` internally.
#[test_case]
fn test_device_binding_rejects_double_grant_until_task_exits() {
    driver_db::reset_bindings_for_test();
    let device = fake_pci_device(0, 5, 0);

    // First "SpawnDriver": claims the device before its task exists yet.
    assert!(
        driver_db::reserve_device_for_test(&device),
        "the first claim on a free device must succeed"
    );

    // A second concurrent/duplicate "SpawnDriver" call for the same device
    // must be rejected while the reservation is still unresolved (task not
    // yet created) — this closes the exact race the module doc describes.
    assert!(
        !driver_db::reserve_device_for_test(&device),
        "a device already reserved must refuse a second concurrent claim"
    );

    // Resolve the first claim to a live task. `spawn_kernel_task` already
    // returns a packed task ID (slot + generation), the same format
    // `confirm_binding`/`release_task` expect.
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    driver_db::confirm_binding(&device, task_id);

    // Now that the reservation is confirmed to a live task, a second
    // "SpawnDriver" for the same device must still be rejected.
    assert!(
        !driver_db::reserve_device_for_test(&device),
        "a device bound to a live task must refuse a second claim"
    );

    // Terminate the owning task — `remove_task` releases the device binding
    // exactly like it releases IRQ bindings (see `irq_bridge::release_task`).
    sched::terminate_task(task_id);

    assert!(
        driver_db::reserve_device_for_test(&device),
        "the device must become claimable again once its owning task has exited"
    );

    driver_db::reset_bindings_for_test();
}

/// Tests that a reservation abandoned by a failed spawn (never confirmed to a
/// task) is released explicitly, without needing a task to terminate.
#[test_case]
fn test_device_reservation_released_on_spawn_failure() {
    driver_db::reset_bindings_for_test();
    let device = fake_pci_device(0, 6, 0);

    assert!(
        driver_db::reserve_device_for_test(&device),
        "the first claim on a free device must succeed"
    );
    assert!(
        !driver_db::reserve_device_for_test(&device),
        "a device already reserved must refuse a second concurrent claim"
    );

    // Simulate SpawnDriver failing after derive_grants (e.g. exec_from_vfs
    // could not load the binary) — the reservation must be released.
    driver_db::release_reservation(&device);

    assert!(
        driver_db::reserve_device_for_test(&device),
        "an abandoned reservation must release the device without a task ever existing"
    );

    driver_db::reset_bindings_for_test();
}

/// Contract: a driver task's exit must disable its bound PCI device's
/// I/O/Memory/Bus-Master decode bits, not just release the binding bookkeeping.
/// Given: A real PCI device (from `pci::init()`'s bus scan) is reserved and
/// bound to a driver task, and its Command Register decode bits are enabled —
/// exactly as `derive_grants`'s Step 4b does for a live `SpawnDriver` call.
/// When: The owning task terminates (`remove_task` -> `driver_db::release_task`).
/// Then: The device's Command Register decode bits are cleared again.
/// Failure Impact: Before this fix, a crashed or killed driver task's device
/// binding was released but its hardware was left fully enabled — a NIC with
/// still-armed DMA descriptors would keep writing into physical memory after
/// `remove_task` frees it back to the PMM for an unrelated task to reuse.
#[test_case]
fn test_release_task_disables_bound_device() {
    pci::init();
    driver_db::reset_bindings_for_test();

    let device = pci::get_devices()
        .into_iter()
        .next()
        .expect("QEMU must expose at least the host bridge on bus 0");

    // SAFETY: reading/writing PCI configuration space via port I/O is safe in
    // Ring 0; offset 0x04 is the standard Command Register.
    let orig_cmd =
        unsafe { pci::pci_config_read(device.bus, device.device, device.function, 0x04) };

    // Simulate what `derive_grants` does once it exclusively reserves a device.
    pci::enable_device(&device);
    let enabled_cmd =
        unsafe { pci::pci_config_read(device.bus, device.device, device.function, 0x04) };
    assert_eq!(
        enabled_cmd & 0x7,
        0x7,
        "enable_device must set I/O/Memory/Bus-Master decode bits"
    );

    assert!(
        driver_db::reserve_device_for_test(&device),
        "device must be free to reserve at test start"
    );
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    driver_db::confirm_binding(&device, task_id);

    sched::terminate_task(task_id);

    let cmd_after_exit =
        unsafe { pci::pci_config_read(device.bus, device.device, device.function, 0x04) };
    assert_eq!(
        cmd_after_exit & 0x7,
        0,
        "a device bound to a task that just exited must have its decode bits cleared"
    );

    // Restore the device's original configuration so later tests observe the
    // same PCI state they would on a fresh boot.
    // SAFETY: restoring the previously-read Command Register value.
    unsafe {
        pci::pci_config_write(device.bus, device.device, device.function, 0x04, orig_cmd);
    }
    driver_db::reset_bindings_for_test();
}

/// Contract: abandoning a device reservation after a failed `SpawnDriver`
/// call must leave the device fully disabled, not just unreserved.
/// Given: A real PCI device is reserved and its Command Register decode bits
/// are enabled — exactly as `derive_grants`'s Step 4b does before the caller's
/// requested grant is validated or the driver binary is loaded.
/// When: The reservation is abandoned via `release_reservation`, mirroring
/// the two failure paths in `syscall_spawn_driver_impl` (a rejected grant
/// request, or a failed `exec_from_vfs`) that run after `derive_grants`
/// already enabled the device but before any task could ever own it.
/// Then: The device's Command Register decode bits are cleared again.
/// Failure Impact: Before this fix, a `SpawnDriver` call that failed after
/// `derive_grants` reserved and enabled a device left that device's hardware
/// permanently decoding MMIO and mastering DMA with no owning task to ever
/// release it.
#[test_case]
fn test_release_reservation_disables_device() {
    pci::init();
    driver_db::reset_bindings_for_test();

    let device = pci::get_devices()
        .into_iter()
        .next()
        .expect("QEMU must expose at least the host bridge on bus 0");

    // SAFETY: reading/writing PCI configuration space via port I/O is safe in
    // Ring 0; offset 0x04 is the standard Command Register.
    let orig_cmd =
        unsafe { pci::pci_config_read(device.bus, device.device, device.function, 0x04) };

    // Simulate `derive_grants`'s Step 4b: reserve the device, then enable it.
    assert!(
        driver_db::reserve_device_for_test(&device),
        "device must be free to reserve at test start"
    );
    pci::enable_device(&device);
    let enabled_cmd =
        unsafe { pci::pci_config_read(device.bus, device.device, device.function, 0x04) };
    assert_eq!(
        enabled_cmd & 0x7,
        0x7,
        "enable_device must set I/O/Memory/Bus-Master decode bits"
    );

    // Simulate SpawnDriver failing after derive_grants (e.g. a rejected grant
    // request, or exec_from_vfs failing to load the binary).
    driver_db::release_reservation(&device);

    let cmd_after_release =
        unsafe { pci::pci_config_read(device.bus, device.device, device.function, 0x04) };
    assert_eq!(
        cmd_after_release & 0x7,
        0,
        "an abandoned reservation must leave the device fully disabled, not just unreserved"
    );

    // Restore the device's original configuration so later tests observe the
    // same PCI state they would on a fresh boot.
    // SAFETY: restoring the previously-read Command Register value.
    unsafe {
        pci::pci_config_write(device.bus, device.device, device.function, 0x04, orig_cmd);
    }
    driver_db::reset_bindings_for_test();
}
