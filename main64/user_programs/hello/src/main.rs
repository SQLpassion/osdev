#![no_std]
#![no_main]

#[path = "../../common/syscall.rs"]
mod syscall;

const HELLO_MSG: &[u8] = b"[ring3] HELLO.BIN launched\n";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // SAFETY:
    // - `HELLO_MSG` is a valid static byte slice in this image.
    // - Kernel validates the user pointer range in the syscall dispatcher.
    unsafe {
        let _ = syscall::write_console(HELLO_MSG.as_ptr(), HELLO_MSG.len());
    }

    syscall::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall::exit()
}
