#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;

use lib_kaos::{println, process};

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("HELLO.BIN launched as a [ring3] task");
    println!("Initializing user heap allocator...");

    println!("User allocator initialized successfully!");

    // Test Box allocation
    let x = Box::new(12345);
    if *x == 12345 {
        println!("Box allocation works (payload = 12345)!");
    } else {
        println!("User allocator failed!");
        process::exit();
    }

    // Test Vec allocation and growth
    let v = alloc::vec![1, 2, 3];

    if v.len() == 3 && v[0] == 1 && v[2] == 3 {
        println!("Vec allocation and growth works (len = 3)!");
    } else {
        println!("User allocator failed!");
        process::exit();
    }

    process::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
