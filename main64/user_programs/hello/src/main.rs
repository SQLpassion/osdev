#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;

use lib_kaos as syscall;
use lib_kaos::heap;

const HELLO_MSG: &[u8] = b"HELLO.BIN launched as a [ring3] task\n";
const ALLOC_START: &[u8] = b"Initializing user heap allocator...\n";
const ALLOC_SUCCESS: &[u8] = b"User allocator initialized successfully!\n";
const BOX_SUCCESS: &[u8] = b"Box allocation works (payload = 12345)!\n";
const VEC_SUCCESS: &[u8] = b"Vec allocation and growth works (len = 3)!\n";
const ALLOC_ERROR: &[u8] = b"User allocator failed!\n";

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    let _ = syscall::write_console(HELLO_MSG);
    let _ = syscall::write_console(ALLOC_START);

    // Initialize the global allocator located in the common heap module.
    // SAFETY:
    // - Setting up the global allocator from single-threaded _start is safe.
    unsafe {
        if heap::init().is_err() {
            let _ = syscall::write_console(ALLOC_ERROR);
            syscall::exit();
        }
    }

    let _ = syscall::write_console(ALLOC_SUCCESS);

    // Test Box allocation
    let x = Box::new(12345);
    if *x == 12345 {
        let _ = syscall::write_console(BOX_SUCCESS);
    } else {
        let _ = syscall::write_console(ALLOC_ERROR);
        syscall::exit();
    }

    // Test Vec allocation and growth
    let v = alloc::vec![1, 2, 3];

    if v.len() == 3 && v[0] == 1 && v[2] == 3 {
        let _ = syscall::write_console(VEC_SUCCESS);
    } else {
        let _ = syscall::write_console(ALLOC_ERROR);
        syscall::exit();
    }

    syscall::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall::exit()
}
