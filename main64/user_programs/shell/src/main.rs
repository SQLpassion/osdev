#![no_std]
#![no_main]

extern crate alloc;

use lib_kaos::{console, fs, process, print, println};

/// Renders the shell welcome banner on startup.
fn print_welcome_banner() {
    println!("========================================");
    println!("    KAOS - Klaus' Operating System");
    println!("        Ring 3 Shell (SHELL.BIN)");
    println!("========================================");
    println!("Type 'help' to see the list of commands.\n");
}

/// The main entry point of the user-space shell.
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    // Step 1: Note that the user heap allocator is now automatically lazy-initialized on the first allocation.

    print_welcome_banner();

    // Step 2: Main command read-eval-print loop
    let mut buf = [0u8; 128];
    loop {
        print!("> ");
        if let Ok(len) = console::readline(&mut buf) {
            if let Ok(line) = core::str::from_utf8(&buf[..len]) {
                execute_command(line);
            } else {
                println!("(invalid UTF-8 input)");
            }
        } else {
            println!("(error reading keyboard input)");
        }
    }
}

/// Parses and dispatches entered shell commands.
fn execute_command(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap();

    match cmd {
        "help" => {
            println!("Commands:");
            println!("  help            - show this help menu");
            println!("  echo <text>     - print the entered text");
            println!("  cls             - clear the console screen");
            println!("  dir             - list directory contents of the FAT12 disk");
            println!("  cat <file>      - read and print the contents of a file");
            println!("  exec <file>     - run a program in the foreground");
            println!("  exit            - exit this shell instance");
            println!("  shutdown        - shutdown the system");
        }
        "echo" => {
            let rest = line[cmd.len()..].trim_start();
            println!("{}", rest);
        }
        "cls" | "clear" => {
            if let Err(err) = console::clear_screen() {
                println!("cls failed: error {:#x}", err);
            }
        }
        "dir" => {
            if let Err(err) = fs::print_root_directory() {
                println!("dir failed: error {:#x}", err);
            }
        }
        "cat" => {
            if let Some(file_name) = parts.next() {
                cat_file(file_name);
            } else {
                println!("Usage: cat <8.3-filename>");
            }
        }
        "exec" => {
            if let Some(file_name) = parts.next() {
                run_program(file_name);
            } else {
                println!("Usage: exec <8.3-filename>");
            }
        }
        "exit" => {
            process::exit();
        }
        "shutdown" => {
            println!("Shutting down KAOS...");
            process::shutdown();
        }
        // Direct execution shortcut for filenames (e.g. typing "hello.bin")
        other if other.ends_with(".bin") || other.ends_with(".BIN") => {
            run_program(other);
        }
        _ => {
            println!("Unknown command: '{}'. Type 'help' for options.", cmd);
        }
    }
}

/// Reads the contents of a file chunk-by-chunk and writes them to the console.
fn cat_file(name: &str) {
    match fs::File::open(name, fs::FileMode::Read) {
        Ok(mut file) => {
            let mut read_buf = [0u8; 128];
            loop {
                match file.read(&mut read_buf) {
                    Ok(0) => break, // EOF reached
                    Ok(bytes_read) => {
                        let _ = console::writeline(&read_buf[..bytes_read]);
                    }
                    Err(err) => {
                        println!("\nError reading file: error {:#x}", err);
                        break;
                    }
                }
            }
        }
        Err(err) => {
            println!("Could not open file '{}': error {:#x}", name, err);
        }
    }
}

/// Launches a user process in the foreground and waits for it to exit.
fn run_program(name: &str) {
    println!("Launching program '{}'...", name);
    match process::exec(name) {
        Ok(pid) => {
            // Wait for the spawned program task to complete.
            // The shell is blocked on the wait queue until the child calls exit.
            if let Err(err) = process::wait(pid) {
                println!("Error waiting for process to finish: error {:#x}", err);
            }
        }
        Err(err) => {
            println!("Failed to execute '{}': error {:#x}", name, err);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("\nShell Panic: {}", _info);
    process::exit()
}
