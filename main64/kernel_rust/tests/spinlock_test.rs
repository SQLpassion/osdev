//! SpinLock Integration Tests

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{pmm, vmm};
use kaos_kernel::sync::spinlock::SpinLock;

/// Entry point for the spinlock integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

#[test_case]
fn test_spinlock_basic_mutation() {
    static LOCK: SpinLock<usize> = SpinLock::new(0);

    {
        let mut guard = LOCK.lock();
        *guard += 1;
    }

    let guard = LOCK.lock();
    assert!(*guard == 1, "spinlock should protect shared state");
}

#[test_case]
fn test_spinlock_preserves_interrupt_state_when_disabled() {
    static LOCK: SpinLock<usize> = SpinLock::new(0);

    interrupts::disable();
    assert!(
        !interrupts::are_enabled(),
        "interrupts should be disabled for this test"
    );

    {
        let mut guard = LOCK.lock();
        *guard += 1;
    }

    assert!(
        !interrupts::are_enabled(),
        "spinlock should not enable interrupts when they were disabled"
    );
}

#[test_case]
fn test_spinlock_preserves_interrupt_state_when_enabled() {
    static LOCK: SpinLock<usize> = SpinLock::new(0);

    interrupts::enable();
    assert!(
        interrupts::are_enabled(),
        "interrupts should be enabled for this test"
    );

    {
        let mut guard = LOCK.lock();
        *guard += 1;
    }

    assert!(
        interrupts::are_enabled(),
        "spinlock should restore enabled interrupts state"
    );

    interrupts::disable();
}
