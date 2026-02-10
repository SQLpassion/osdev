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

/// Contract: keyboard e2e scancode to char.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "keyboard e2e scancode to char".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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

/// Contract: keyboard e2e shift uppercase.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "keyboard e2e shift uppercase".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
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

/// Contract: keyboard init clears all buffers and modifiers.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "keyboard init clears all buffers and modifiers".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_keyboard_init_clears_all_buffers_and_modifiers() {
    keyboard::init();

    // Dirty state: enable shift, decode one character, leave remaining input.
    keyboard::enqueue_raw_scancode(0x2a); // left shift make
    keyboard::enqueue_raw_scancode(0x1e); // 'a'
    keyboard::enqueue_raw_scancode(0xaa); // left shift break
    keyboard::enqueue_raw_scancode(0x1e); // 'a'
    assert!(
        keyboard::process_pending_scancodes(),
        "precondition: dirty sequence must be processed"
    );

    assert!(
        keyboard::read_char() == Some(b'A'),
        "shifted character should be present before reset"
    );

    keyboard::init();
    assert!(
        keyboard::read_char().is_none(),
        "init must clear decoded input buffer"
    );

    keyboard::enqueue_raw_scancode(0x1e);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration should process fresh scancode after init"
    );
    assert!(
        keyboard::read_char() == Some(b'a'),
        "after init, modifiers must be reset and decode lowercase"
    );
}

/// Contract: process pending scancodes without input does not mutate state.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "process pending scancodes without input does not mutate state".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_process_pending_scancodes_without_input_does_not_mutate_state() {
    keyboard::init();

    assert!(
        !keyboard::process_pending_scancodes(),
        "processing without input must report no work"
    );
    assert!(
        keyboard::read_char().is_none(),
        "processing without input must not create decoded characters"
    );
}

/// Contract: keyboard multiple init calls are idempotent.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "keyboard multiple init calls are idempotent".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_keyboard_multiple_init_calls_are_idempotent() {
    keyboard::init();

    keyboard::enqueue_raw_scancode(0x1e);
    assert!(
        keyboard::process_pending_scancodes(),
        "precondition: queued scancode should be processed"
    );
    assert!(
        keyboard::read_char() == Some(b'a'),
        "precondition: one decoded character should be available"
    );

    keyboard::init();
    keyboard::init();

    assert!(
        keyboard::read_char().is_none(),
        "repeated init must leave buffer empty"
    );

    keyboard::enqueue_raw_scancode(0x1e);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration should still work after repeated init"
    );
    assert!(
        keyboard::read_char() == Some(b'a'),
        "decode path must remain correct after repeated init"
    );
}

/// Contract: keyboard state does not leak between test cases a dirty state.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "keyboard state does not leak between test cases a dirty state".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_keyboard_state_does_not_leak_between_test_cases_a_dirty_state() {
    keyboard::init();

    // Dirty global modifier state intentionally.
    keyboard::enqueue_raw_scancode(0x3a); // caps lock toggle on
    keyboard::enqueue_raw_scancode(0x1e); // 'a'
    assert!(
        keyboard::process_pending_scancodes(),
        "dirty-state test should process queued scancodes"
    );
    assert!(
        keyboard::read_char() == Some(b'A'),
        "caps lock should uppercase before reset"
    );
}

/// Contract: keyboard state does not leak between test cases b after init.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "keyboard state does not leak between test cases b after init".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_keyboard_state_does_not_leak_between_test_cases_b_after_init() {
    keyboard::init();

    keyboard::enqueue_raw_scancode(0x1e);
    assert!(
        keyboard::process_pending_scancodes(),
        "fresh scancode should be processed after init"
    );
    assert!(
        keyboard::read_char() == Some(b'a'),
        "init must reset global keyboard state between test cases"
    );
}
