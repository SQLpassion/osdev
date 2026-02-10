//! Keyboard end-to-end integration tests.
//!
//! Verifies the pipeline:
//! raw scancode enqueue -> keyboard worker decode -> character read API.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::drivers::keyboard;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

#[test_case]
fn test_keyboard_e2e_scancode_to_char() {
    keyboard::init();

    // Make code for 'a'
    keyboard::enqueue_raw_scancode(0x1e);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration should process queued scancode"
    );
    assert!(
        keyboard::read_char() == Some(b'a'),
        "scancode 0x1e should decode to 'a'"
    );
}

#[test_case]
fn test_keyboard_e2e_shift_uppercase() {
    keyboard::init();

    // Left shift make, 'a' make, left shift break.
    keyboard::enqueue_raw_scancode(0x2a);
    keyboard::enqueue_raw_scancode(0x1e);
    keyboard::enqueue_raw_scancode(0xaa);

    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration should process shift + key sequence"
    );
    assert!(
        keyboard::read_char() == Some(b'A'),
        "shift + 'a' should decode to uppercase 'A'"
    );
    assert!(
        keyboard::read_char().is_none(),
        "only one printable character should be produced"
    );
}
