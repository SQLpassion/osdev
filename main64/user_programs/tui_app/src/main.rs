#![no_std]
#![no_main]

extern crate alloc;

mod app;

use lib_kaos::process;

/// Entry point for the TUI user-space application.
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    app::run_demo();
    process::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
