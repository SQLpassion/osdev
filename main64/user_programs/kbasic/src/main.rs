#![no_std]
#![no_main]

extern crate alloc;

mod token;
mod interpreter;

use lib_kaos::{print, println, console};
use token::tokenize_line;
use interpreter::Interpreter;

/// The main entry point of the user-space basic interpreter.
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("KAOS BASIC Interpreter (Ring 3)");
    println!("Type your commands (e.g. LET A = 5, PRINT A, IF A > 3 THEN PRINT \"Greater\")");
    println!("Type 'exit' to quit.\n");

    let mut interpreter = Interpreter::new();
    let mut buf = [0u8; 128];

    loop {
        print!("KBASIC > ");
        if let Ok(len) = console::readline(&mut buf) {
            if let Ok(line) = core::str::from_utf8(&buf[..len]) {
                let line_trimmed = line.trim();
                // Check for exit
                let mut is_exit = false;
                if line_trimmed.len() == 4 {
                    let mut bytes = [0u8; 4];
                    bytes.copy_from_slice(line_trimmed.as_bytes());
                    for b in &mut bytes {
                        *b = b.to_ascii_lowercase();
                    }
                    if &bytes == b"exit" {
                        is_exit = true;
                    }
                }
                if is_exit {
                    break;
                }
                let tokens = tokenize_line(line_trimmed);
                interpreter.execute(&tokens);
            } else {
                println!("(invalid UTF-8 input)");
            }
        } else {
            println!("(error reading input)");
        }
    }

    lib_kaos::process::exit();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    lib_kaos::process::exit()
}
