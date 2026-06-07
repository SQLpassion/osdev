#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write;

#[path = "../../common/syscall.rs"]
mod syscall;

#[path = "../../common/heap.rs"]
mod heap;

/// Proxy struct to implement `core::fmt::Write` formatting for console output.
struct ConsoleWriter;

impl core::fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let _ = syscall::write_console(s.as_bytes());
        Ok(())
    }
}

macro_rules! print {
    ($($arg:tt)*) => {{
        let mut writer = ConsoleWriter;
        let _ = write!(writer, $($arg)*);
    }};
}

macro_rules! println {
    () => {{
        let _ = syscall::write_console(b"\n");
    }};
    ($($arg:tt)*) => {{
        let mut writer = ConsoleWriter;
        let _ = writeln!(writer, $($arg)*);
    }};
}

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
    // Step 1: Initialize User Heap Allocator so we can use dynamic vectors/strings.
    // SAFETY:
    // - Initializing the user allocator from the single-threaded shell entry point is safe.
    unsafe {
        if heap::init().is_err() {
            let _ = syscall::write_console(b"Fatal: User heap allocator failed to initialize.\n");
            syscall::exit();
        }
    }

    print_welcome_banner();

    // Step 2: Main command read-eval-print loop
    let mut buf = [0u8; 128];
    loop {
        print!("> ");
        if let Ok(len) = syscall::user_readline(&mut buf) {
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
            println!("  shutdown        - shutdown the system");
        }
        "echo" => {
            let rest = line[cmd.len()..].trim_start();
            println!("{}", rest);
        }
        "cls" | "clear" => {
            if let Err(err) = syscall::clear_screen() {
                println!("cls failed: error {:#x}", err);
            }
        }
        "dir" => {
            if let Err(err) = syscall::print_root_directory() {
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
        "shutdown" => {
            println!("Shutting down KAOS...");
            syscall::shutdown();
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
    match syscall::File::open(name.as_bytes(), syscall::FileMode::Read) {
        Ok(mut file) => {
            let mut read_buf = [0u8; 128];
            loop {
                match file.read(&mut read_buf) {
                    Ok(0) => break, // EOF reached
                    Ok(bytes_read) => {
                        let _ = syscall::write_console(&read_buf[..bytes_read]);
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
    match syscall::exec(name.as_bytes()) {
        Ok(pid) => {
            // Wait for the spawned program task to complete.
            // The shell is blocked on the wait queue until the child calls exit.
            if let Err(err) = syscall::wait(pid) {
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
    syscall::exit()
}
