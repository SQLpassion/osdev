//! High-precision Time Driver Integration Tests

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::drivers::time::{self, DateTime};
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the time integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);

    // Initialize time driver
    time::init();

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

/// Contract: get_time returns a valid DateTime structure.
/// Given: Time driver has been initialized.
/// When: get_time is called.
/// Then: Year must be at least 2000.
#[test_case]
fn test_get_time_returns_valid_struct() {
    let t = time::get_time();
    assert!(t.year >= 2000, "Year should be reasonable");
    assert!(t.month >= 1 && t.month <= 12, "Month should be valid");
    assert!(t.day >= 1 && t.day <= 31, "Day should be valid");
    assert!(t.hour < 24, "Hour should be valid");
    assert!(t.minute < 60, "Minute should be valid");
    assert!(t.second < 60, "Second should be valid");
}

/// Contract: rdtsc returns non-zero values.
/// Given: The system is running.
/// When: rdtsc is called twice.
/// Then: The second call should be greater than or equal to the first call.
#[test_case]
fn test_rdtsc_increases() {
    let t1 = time::rdtsc();
    let t2 = time::rdtsc();
    assert!(t2 >= t1, "rdtsc should be monotonic");
}

/// Contract: calibrate_tsc returns a positive, plausible frequency.
/// Given: The PIT is accessible and the TSC is running.
/// When: calibrate_tsc is called.
/// Then: The result is non-zero and within a sane CPU frequency range.
#[test_case]
fn test_calibrate_tsc_returns_plausible_value() {
    let ticks_per_us = time::calibration::calibrate_tsc();

    assert!(
        ticks_per_us > 0,
        "calibrated TSC frequency must be positive"
    );
    assert!(
        ticks_per_us >= 10 && ticks_per_us <= 20_000,
        "calibrated TSC frequency {} is outside plausible CPU range",
        ticks_per_us
    );
}

/// Contract: calibrate_tsc does not hang on a stuck PIT counter.
/// Given: The function is bounded by a timeout.
/// When: calibrate_tsc is called.
/// Then: It returns within a bounded number of iterations (observed by the
/// test completing) and yields either a measured value or the safe fallback.
#[test_case]
fn test_calibrate_tsc_is_bounded() {
    // The bounded-loop fix is verified implicitly: if calibrate_tsc could
    // hang forever, this test kernel would never reach the assertion.
    let ticks_per_us = time::calibration::calibrate_tsc();
    assert!(
        ticks_per_us > 0,
        "calibrate_tsc must return a positive frequency even on timeout fallback"
    );
}

/// Contract: DateTime add_seconds correctly propagates overflow.
/// Given: A DateTime value at the edge of rollover.
/// When: add_seconds is called.
/// Then: Rollover into next minute, hour, day, month, and year must be correct.
#[test_case]
fn test_datetime_add_seconds_rollover() {
    let mut dt = DateTime {
        year: 2026,
        month: 12,
        day: 31,
        hour: 23,
        minute: 59,
        second: 59,
    };
    dt.add_seconds(1);
    assert_eq!(dt.year, 2027);
    assert_eq!(dt.month, 1);
    assert_eq!(dt.day, 1);
    assert_eq!(dt.hour, 0);
    assert_eq!(dt.minute, 0);
    assert_eq!(dt.second, 0);
}

/// Contract: GET_TIME syscall works correctly via user-space wrapper.
/// Given: The system is running in the test environment.
/// When: sys_get_time is called with a mutable UserDateTime reference.
/// Then: The returned struct must have valid calendar ranges.
#[test_case]
fn test_syscall_get_time() {
    use kaos_kernel::syscall::user::sys_get_time;
    use kaos_kernel::syscall::UserDateTime;

    // Step 1: Allocate a writable user page because the syscall contract
    // requires output pointers to reference present user-accessible memory.
    let user_va = vmm::USER_HEAP_BASE + 0x3000;
    let phys = vmm::page_table::alloc_frame_phys().unwrap();
    let pfn = vmm::page_table::phys_to_pfn(phys);
    vmm::map_user_page(user_va, pfn, true).unwrap();

    // SAFETY:
    // - `user_va` is backed by a present, writable user page mapped above.
    let res = unsafe { sys_get_time(user_va as *mut UserDateTime) };
    assert!(res.is_ok(), "sys_get_time syscall should succeed");

    // SAFETY:
    // - The successful syscall initialized the UserDateTime at `user_va`.
    let udt = unsafe { core::ptr::read(user_va as *const UserDateTime) };
    assert!(udt.year >= 2000, "Year should be reasonable");
}
