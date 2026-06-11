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

/// Contract: scancode 0x29 (caret/^ key) decodes to '^' without shift.
/// Given: The subsystem is initialized.
/// When: The make code 0x29 is enqueued and processed.
/// Then: read_char returns Some(b'^') – the unshifted character on the German
///       QWERTZ layout for that physical key.
/// Failure Impact: Regression in QWERTZ special-character mapping.
#[test_case]
fn test_qwertz_caret_key_unshifted() {
    keyboard::init();

    // 0x29 = caret/circumflex key (^)
    keyboard::enqueue_raw_scancode(0x29);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process scancode 0x29"
    );
    assert!(
        keyboard::read_char() == Some(b'^'),
        "scancode 0x29 without shift must decode to '^'"
    );
}

/// Contract: scancode 0x0C (ß/? key) produces no character unshifted.
/// Given: The subsystem is initialized.
/// When: The make code 0x0C is enqueued and processed.
/// Then: read_char returns None – ß has no ASCII representation and must be
///       silently ignored by the driver.
/// Failure Impact: Regression in QWERTZ special-character mapping (ß key).
#[test_case]
fn test_qwertz_sz_key_unshifted_yields_no_char() {
    keyboard::init();

    // 0x0C = ß/? key on German QWERTZ; ß has no ASCII code point.
    keyboard::enqueue_raw_scancode(0x0c);
    keyboard::process_pending_scancodes();
    assert!(
        keyboard::read_char().is_none(),
        "scancode 0x0C (ß) must not produce an ASCII character"
    );
}

/// Contract: scancode 0x0C with shift decodes to '?'.
/// Given: The subsystem is initialized with LShift held.
/// When: The make code 0x0C is enqueued after a shift make code.
/// Then: read_char returns Some(b'?') – Shift+ß = ? on German QWERTZ.
/// Failure Impact: Regression in QWERTZ special-character mapping (ß/? key).
#[test_case]
fn test_qwertz_sz_key_shifted_yields_question_mark() {
    keyboard::init();

    // LShift make + 0x0C make + LShift break.
    keyboard::enqueue_raw_scancode(0x2a);
    keyboard::enqueue_raw_scancode(0x0c);
    keyboard::enqueue_raw_scancode(0xaa);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process shift + 0x0C sequence"
    );
    assert!(
        keyboard::read_char() == Some(b'?'),
        "shift + scancode 0x0C must decode to '?'"
    );
}

/// Contract: scancode 0x1B (+ /* key) decodes to '+' without shift.
/// Given: The subsystem is initialized.
/// When: The make code 0x1B is enqueued and processed.
/// Then: read_char returns Some(b'+').
/// Failure Impact: Regression in QWERTZ special-character mapping (+/* key).
#[test_case]
fn test_qwertz_plus_key_unshifted() {
    keyboard::init();

    keyboard::enqueue_raw_scancode(0x1b);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process scancode 0x1B"
    );
    assert!(
        keyboard::read_char() == Some(b'+'),
        "scancode 0x1B without shift must decode to '+'"
    );
}

/// Contract: scancode 0x1B with shift decodes to '*'.
/// Given: The subsystem is initialized with LShift held.
/// When: The make code 0x1B is enqueued.
/// Then: read_char returns Some(b'*') – Shift++ = * on German QWERTZ.
/// Failure Impact: Regression in QWERTZ special-character mapping (+/* key).
#[test_case]
fn test_qwertz_plus_key_shifted_yields_star() {
    keyboard::init();

    keyboard::enqueue_raw_scancode(0x2a);
    keyboard::enqueue_raw_scancode(0x1b);
    keyboard::enqueue_raw_scancode(0xaa);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process shift + 0x1B sequence"
    );
    assert!(
        keyboard::read_char() == Some(b'*'),
        "shift + scancode 0x1B must decode to '*'"
    );
}

/// Contract: scancode 0x2B (# key) decodes to '#' without shift.
/// Given: The subsystem is initialized.
/// When: The make code 0x2B is enqueued and processed.
/// Then: read_char returns Some(b'#').
/// Failure Impact: Regression in QWERTZ special-character mapping (# key).
#[test_case]
fn test_qwertz_hash_key_unshifted() {
    keyboard::init();

    keyboard::enqueue_raw_scancode(0x2b);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process scancode 0x2B"
    );
    assert!(
        keyboard::read_char() == Some(b'#'),
        "scancode 0x2B without shift must decode to '#'"
    );
}

/// Contract: scancode 0x56 (ISO key <>) decodes to '<' without shift.
/// Given: The subsystem is initialized.
/// When: The make code 0x56 is enqueued and processed.
/// Then: read_char returns Some(b'<') – the ISO key between LShift and Y.
/// Failure Impact: Regression in QWERTZ special-character mapping (ISO <> key).
#[test_case]
fn test_qwertz_iso_key_unshifted_yields_less_than() {
    keyboard::init();

    // 0x56 = the extra ISO key (between LShift and Z/Y on 102-key QWERTZ).
    keyboard::enqueue_raw_scancode(0x56);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process scancode 0x56"
    );
    assert!(
        keyboard::read_char() == Some(b'<'),
        "scancode 0x56 without shift must decode to '<'"
    );
}

/// Contract: scancode 0x56 with shift decodes to '>'.
/// Given: The subsystem is initialized with LShift held.
/// When: The make code 0x56 is enqueued.
/// Then: read_char returns Some(b'>') – Shift+< = > on German QWERTZ.
/// Failure Impact: Regression in QWERTZ special-character mapping (ISO <> key).
#[test_case]
fn test_qwertz_iso_key_shifted_yields_greater_than() {
    keyboard::init();

    keyboard::enqueue_raw_scancode(0x2a);
    keyboard::enqueue_raw_scancode(0x56);
    keyboard::enqueue_raw_scancode(0xaa);
    assert!(
        keyboard::process_pending_scancodes(),
        "worker iteration must process shift + 0x56 sequence"
    );
    assert!(
        keyboard::read_char() == Some(b'>'),
        "shift + scancode 0x56 must decode to '>'"
    );
}

/// Contract: clear_buffers drains all input from legacy and key buffers.
/// Given: The keyboard driver is initialized, and a key event has been processed into the buffers.
/// When: clear_buffers is called.
/// Then: subsequent reads from either legacy or key buffer return None.
/// Failure Impact: Stale key events can leak between different application contexts (like TUI and REPL).
#[test_case]
fn test_keyboard_clear_buffers_drains_all_inputs() {
    keyboard::init();

    // Enqueue 'q' make code (0x10)
    keyboard::enqueue_raw_scancode(0x10);
    assert!(
        keyboard::process_pending_scancodes(),
        "precondition: keyboard worker should process the scancode"
    );

    // Verify both buffers are populated
    assert!(keyboard::read_key().is_some(), "key buffer should not be empty");
    
    // Repopulate (reading consumed the event)
    keyboard::enqueue_raw_scancode(0x10);
    keyboard::process_pending_scancodes();

    // Call clear_buffers
    keyboard::clear_buffers();

    // Verify both are now empty
    assert!(keyboard::read_char().is_none(), "legacy buffer must be empty after clear_buffers");
    assert!(keyboard::read_key().is_none(), "key buffer must be empty after clear_buffers");
}

