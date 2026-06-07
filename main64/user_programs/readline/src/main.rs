#![no_std]
#![no_main]

use lib_kaos::{console, process, print, println};

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    let mut line = [0u8; 128];

    print!("Enter your name: ");

    match console::readline(&mut line) {
        Ok(line_len) => {
            if let Ok(name) = core::str::from_utf8(&line[..line_len]) {
                println!("Your name is: {}", name);
            } else {
                println!("Your name is: (invalid UTF-8)");
            }
        }
        Err(_) => {
            println!("READLINE.BIN: readline failed");
        }
    }

    process::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
