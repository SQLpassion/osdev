#![no_std]
#![no_main]

#[path = "../../common/syscall.rs"]
mod syscall;

const HELLO_MSG: &[u8] = b"HELLO.BIN launched as a [ring3] task\n";

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    let _ = syscall::write_console(HELLO_MSG);

    syscall::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall::exit()
}
