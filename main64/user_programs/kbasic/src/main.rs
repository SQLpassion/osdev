#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

mod token;
mod interpreter;

#[cfg(not(test))]
use lib_kaos::{print, println, console};
#[cfg(not(test))]
use token::tokenize_line;
#[cfg(not(test))]
use interpreter::Interpreter;

/// The main entry point of the user-space basic interpreter.
#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("KAOS BASIC Interpreter (Ring 3)");
    println!("Type your commands (e.g. LET A = 5, PRINT A, IF A > 3 THEN PRINT \"Greater\")");
    println!("Type '.exec <filename>' to execute a BASIC script.");
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
                if let Some(stripped) = line_trimmed.strip_prefix(".exec ") {
                    let filename = stripped.trim();
                    match lib_kaos::fs::File::open(filename, lib_kaos::fs::FileMode::Read) {
                        Ok(mut file) => {
                            let mut file_content = alloc::vec::Vec::new();
                            let mut chunk = [0u8; 256];
                            let mut read_failed = false;
                            loop {
                                match file.read(&mut chunk) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        file_content.extend_from_slice(&chunk[..n]);
                                    }
                                    Err(err) => {
                                        println!("(error reading file: error {:#x})", err);
                                        read_failed = true;
                                        break;
                                    }
                                }
                            }
                            if !read_failed {
                                if let Ok(content) = core::str::from_utf8(&file_content) {
                                    interpreter.execute_script(content);
                                } else {
                                    println!("(file contains invalid UTF-8)");
                                }
                            }
                        }
                        Err(err) => {
                            println!("(could not open file '{}': error {:#x})", filename, err);
                        }
                    }
                    continue;
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    lib_kaos::process::exit()
}
