//! Screen/VGA driver integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::fmt::Write;
use core::panic::PanicInfo;
use kaos_kernel::drivers::screen::{with_screen, Color, PanicScreenWriter, Screen};

const VGA_BUFFER: usize = 0xFFFF8000000B8000;
const VGA_COLS: usize = 80;
const VGA_ROWS: usize = 25;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    // Step 1: Initialize memory management and heap to support dynamic allocations.
    kaos_kernel::memory::pmm::init(false);
    kaos_kernel::arch::interrupts::init();
    kaos_kernel::memory::vmm::init(false);
    kaos_kernel::memory::heap::init(false);

    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: print char wrap at last cell keeps cursor in bounds.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "print char wrap at last cell keeps cursor in bounds".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_print_char_wrap_at_last_cell_keeps_cursor_in_bounds() {
    let mut screen = Screen::new();
    screen.clear();

    screen.set_cursor(VGA_ROWS - 1, VGA_COLS - 1);
    screen.print_char(b'X');
    screen.print_char(b'Y');

    let (row, col) = screen.get_cursor();
    assert!(row < VGA_ROWS, "cursor row must stay in bounds after wrap");
    assert!(col < VGA_COLS, "cursor col must stay in bounds after wrap");
    assert!(
        row == VGA_ROWS - 1 && col == 1,
        "after writing at last cell and one more char, cursor should be at last row col 1"
    );
}

/// Contract: print char wrap writes to last row after scroll.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "print char wrap writes to last row after scroll".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_print_char_wrap_writes_to_last_row_after_scroll() {
    let mut screen = Screen::new();
    screen.clear();

    screen.set_cursor(VGA_ROWS - 1, VGA_COLS - 1);
    screen.print_char(b'X');
    screen.print_char(b'Y');

    let cell = VGA_BUFFER + ((VGA_ROWS - 1) * VGA_COLS) * 2;
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `cell` points to VGA text MMIO for row 24 col 0.
    // - Volatile read is required for MMIO.
    let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
    assert!(
        ch == b'Y',
        "wrapped character should be written at last row col 0"
    );
}

/// Contract: print str writes contiguous progress bar pattern.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "print str writes contiguous progress bar pattern".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_print_str_writes_contiguous_progress_bar_pattern() {
    let mut screen = Screen::new();
    screen.clear();

    let row = 5usize;
    let col = 10usize;
    let pattern = b"[#####     ]";

    screen.set_cursor(row, col);
    screen.print_str(core::str::from_utf8(pattern).expect("pattern must be valid ASCII"));

    for (idx, expected) in pattern.iter().enumerate() {
        let cell = VGA_BUFFER + ((row * VGA_COLS + col + idx) * 2);
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `cell` points to VGA text MMIO for the selected row/column.
        // - Volatile read is required for MMIO.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(
            ch == *expected,
            "VGA cell must contain the expected progress bar byte"
        );
    }
}

/// Contract: print str can cover a complete VGA text row.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "print str can cover a complete VGA text row".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_print_str_can_cover_complete_vga_text_row() {
    let mut screen = Screen::new();
    screen.clear();

    let row = 8usize;
    let mut full_row = [b'.'; VGA_COLS];
    full_row[0] = b'X';
    let full_row_str = core::str::from_utf8(&full_row).expect("full-row bytes must be valid ASCII");

    screen.set_cursor(row, 0);
    screen.print_str(full_row_str);

    for (idx, expected) in full_row.iter().enumerate() {
        let cell = VGA_BUFFER + ((row * VGA_COLS + idx) * 2);
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `cell` points to VGA text MMIO for the selected row/column.
        // - Volatile read is required for MMIO.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(
            ch == *expected,
            "VGA row write must preserve each byte across full width"
        );
    }
}

/// Contract: full-width row can include label in first column and fill afterwards.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "full-width row can include label in first column and fill afterwards".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_full_width_row_can_include_label_and_fill_afterwards() {
    let mut screen = Screen::new();
    screen.clear();

    let row = 9usize;
    let mut full_row = [b'.'; VGA_COLS];
    full_row[0] = b'A';
    for item in full_row.iter_mut().take(41).skip(1) {
        *item = b'#';
    }
    let full_row_str = core::str::from_utf8(&full_row).expect("full-row bytes must be valid ASCII");

    screen.set_cursor(row, 0);
    screen.print_str(full_row_str);

    for (idx, expected) in full_row.iter().enumerate() {
        let cell = VGA_BUFFER + ((row * VGA_COLS + idx) * 2);
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `cell` points to VGA text MMIO for the selected row/column.
        // - Volatile read is required for MMIO.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(
            ch == *expected,
            "VGA row write with label + fill pattern must match expected bytes"
        );
    }
}

/// Contract: full-width row rewrite updates visible progress content.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "full-width row rewrite updates visible progress content".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_full_width_row_rewrite_updates_visible_progress_content() {
    let mut screen = Screen::new();
    screen.clear();

    let row = 10usize;
    let mut first = [b'.'; VGA_COLS];
    let mut second = [b'.'; VGA_COLS];
    first[0] = b'B';
    second[0] = b'B';
    for item in first.iter_mut().take(21).skip(1) {
        *item = b'#';
    }
    for item in second.iter_mut().take(41).skip(1) {
        *item = b'#';
    }

    let first_str = core::str::from_utf8(&first).expect("first row bytes must be valid ASCII");
    let second_str = core::str::from_utf8(&second).expect("second row bytes must be valid ASCII");

    screen.set_cursor(row, 0);
    screen.print_str(first_str);
    screen.set_cursor(row, 0);
    screen.print_str(second_str);

    for (idx, expected) in second.iter().enumerate() {
        let cell = VGA_BUFFER + ((row * VGA_COLS + idx) * 2);
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `cell` points to VGA text MMIO for the selected row/column.
        // - Volatile read is required for MMIO.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(
            ch == *expected,
            "rewritten VGA row must match the latest progress pattern"
        );
    }
}

/// Contract: with_screen reuses the global screen cursor state across calls.
#[test_case]
fn test_with_screen_keeps_global_cursor_between_calls() {
    with_screen(|screen| {
        screen.clear();
        screen.set_cursor(0, 0);
        screen.print_char(b'A');
    });

    with_screen(|screen| {
        let (row, col) = screen.get_cursor();
        assert!(row == 0, "row must remain on first line after one byte");
        assert!(
            col == 1,
            "cursor must advance and persist across with_screen calls"
        );
        screen.clear();
    });
}

/// Contract: panic screen writer is lock-free and writes directly to VGA buffer.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "panic screen writer is lock-free and writes directly to VGA buffer".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_panic_screen_writer_writes_without_global_lock() {
    with_screen(|screen| screen.clear());

    let mut panic_writer = PanicScreenWriter::new(Color::White, Color::Blue);
    panic_writer.clear();
    write!(panic_writer, "PANIC").expect("panic writer should support fmt::Write");

    let expected = b"PANIC";
    for (idx, byte) in expected.iter().enumerate() {
        let cell = VGA_BUFFER + idx * 2;
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `cell` addresses the first-row VGA MMIO character cells.
        // - Volatile read is required for MMIO.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(
            ch == *byte,
            "panic writer must write expected byte sequence"
        );
    }
}

/// Contract: print str with ASCII hyphen renders the character correctly.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "print str with ASCII hyphen renders the character correctly".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_print_str_with_hyphen_renders_correctly() {
    let mut screen = Screen::new();
    screen.clear();

    let row = 11usize;
    let col = 5usize;
    let test_str = "Physical Memory Manager (PMM) - free-list allocator";

    screen.set_cursor(row, col);
    screen.print_str(test_str);

    for (idx, &expected_byte) in test_str.as_bytes().iter().enumerate() {
        let cell = VGA_BUFFER + ((row * VGA_COLS + col + idx) * 2);
        // SAFETY:
        // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
        // - `cell` points to a calculated in-bounds address within the VGA text MMIO buffer.
        // - Volatile read is required for MMIO to read the character directly from the VGA memory.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(
            ch == expected_byte,
            "VGA cell must contain the expected character byte"
        );
    }
}

/// Contract: console::init initializes GLOBAL_CONSOLE and with_console delegates successfully.
/// Given: The subsystem is initialized with the explicit preconditions in this test body.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold, proving dynamic console routing is functional.
#[test_case]
fn test_dynamic_console_initialization_and_routing() {
    // Step 1: Initialize the console module in VGA text mode.
    kaos_kernel::console::init(kaos_kernel::boot_info::VideoModeType::VgaText);

    // Step 2: Use the abstraction to clear the screen and write a test string.
    kaos_kernel::console::with_console(|console| {
        console.clear();
        console.set_cursor(5, 10);
        console.print_str("DYNAMIC TUI TEST");
    });

    // Step 3: Verify the string was written to the VGA buffer.
    let expected = b"DYNAMIC TUI TEST";
    for (idx, &byte) in expected.iter().enumerate() {
        let cell = VGA_BUFFER + ((5 * VGA_COLS + 10 + idx) * 2);
        // SAFETY:
        // - Raw pointer read is required to verify actual hardware state.
        // - Address is mapped and valid for read.
        let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
        assert!(ch == byte, "dynamic console must write expected character");
    }
}

/// Contract: FramebufferConsole fallback creation and basic cursor advancement.
/// Given: The subsystem is initialized in fallback mode (when BOOT_INFO_PTR is 0).
/// When: We write a character.
/// Then: The cursor coordinates advance correctly.
#[test_case]
fn test_framebuffer_console_fallback_creation_and_cursor() {
    use kaos_kernel::console::KernelConsole;

    let mut fb_console = kaos_kernel::console::FramebufferConsole::new();
    assert_eq!(fb_console.get_cursor(), (0, 0));

    // Write a character and check cursor advancement.
    fb_console.print_char(b'A');
    assert_eq!(fb_console.get_cursor(), (0, 1));
}

/// Contract: framebuffer console maps TUI CP437 bytes to non-fallback glyphs.
/// Given: The framebuffer font selector receives bytes emitted by Ring-3 TUI widgets.
/// When: The glyphs are selected for box, table, dialog, and progress-bar characters.
/// Then: The renderer must not replace those bytes with the visible '?' fallback glyph.
#[test_case]
fn test_framebuffer_console_maps_tui_cp437_bytes_to_glyphs() {
    let fallback = kaos_kernel::console::FramebufferConsole::glyph_for_byte(b'?');
    let cp437_bytes = [
        0xB0, 0xB3, 0xB4, 0xB5, 0xBA, 0xBB, 0xBC, 0xBF, 0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6,
        0xC8, 0xC9, 0xCD, 0xD9, 0xDA, 0xDB,
    ];

    for &byte in cp437_bytes.iter() {
        let glyph = kaos_kernel::console::FramebufferConsole::glyph_for_byte(byte);

        assert!(
            glyph != fallback,
            "supported CP437 byte must not render as fallback '?'"
        );
    }
}

/// Contract: framebuffer console keeps ASCII glyph selection unchanged.
/// Given: The framebuffer font selector receives a plain ASCII byte.
/// When: The glyph is selected for that byte.
/// Then: It must differ from the CP437 line glyph used for the TUI border path.
#[test_case]
fn test_framebuffer_console_keeps_ascii_glyphs_distinct_from_cp437_lines() {
    let ascii_a = kaos_kernel::console::FramebufferConsole::glyph_for_byte(b'A');
    let horizontal_line = kaos_kernel::console::FramebufferConsole::glyph_for_byte(0xC4);

    assert!(
        ascii_a != horizontal_line,
        "ASCII glyph selection must remain distinct from CP437 line drawing"
    );
}

/// Contract: framebuffer console keeps unsupported extended bytes visibly marked.
/// Given: The framebuffer font selector receives an extended byte without a dedicated glyph.
/// When: The glyph is selected directly from the 256-entry font table.
/// Then: It must resolve to the same visible '?' fallback glyph as before.
#[test_case]
fn test_framebuffer_console_maps_unsupported_extended_byte_to_fallback_glyph() {
    let fallback = kaos_kernel::console::FramebufferConsole::glyph_for_byte(b'?');
    let unsupported = kaos_kernel::console::FramebufferConsole::glyph_for_byte(0x80);

    assert!(
        unsupported == fallback,
        "unsupported extended byte must render as fallback '?'"
    );
}
