//! Screen/VGA driver integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::drivers::screen::Screen;

const VGA_BUFFER: usize = 0xFFFF8000000B8000;
const VGA_COLS: usize = 80;
const VGA_ROWS: usize = 25;

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

#[test_case]
fn test_print_char_wrap_writes_to_last_row_after_scroll() {
    let mut screen = Screen::new();
    screen.clear();

    screen.set_cursor(VGA_ROWS - 1, VGA_COLS - 1);
    screen.print_char(b'X');
    screen.print_char(b'Y');

    let cell = VGA_BUFFER + ((VGA_ROWS - 1) * VGA_COLS) * 2;
    // SAFETY:
    // - `cell` points to VGA text MMIO for row 24 col 0.
    // - Volatile read is required for MMIO.
    let ch = unsafe { core::ptr::read_volatile(cell as *const u8) };
    assert!(ch == b'Y', "wrapped character should be written at last row col 0");
}
