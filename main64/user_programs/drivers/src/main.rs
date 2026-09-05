#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

mod load_driver;

#[cfg(not(test))]
use lib_kaos::{console, print, println, process};

/// Parsed shell-style command line for the `drivers` REPL.
///
/// Only the commands implemented so far have a dedicated variant; anything
/// else falls into [`Command::Unknown`]. This mirrors the shell's own
/// `execute_command` dispatch, but factored out as a pure function so it is
/// unit-testable without a scheduler, VFS, or real syscalls (see this
/// project's `resolve_driver_filename` convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command<'a> {
    /// Blank input (whitespace-only or empty) — the REPL loop silently
    /// re-prompts, matching the shell's own behavior.
    Empty,
    /// `help` — print the list of available commands. Any extra words on
    /// the line are ignored, same as the shell's own `help`.
    Help,
    /// `list` — enumerate currently loaded drivers. Any extra words on the
    /// line are ignored.
    List,
    /// `load <name>` — the filename argument, or `None` if omitted.
    Load(Option<&'a str>),
    /// `unload <name>` — the driver name argument, or `None` if omitted.
    Unload(Option<&'a str>),
    /// `exit` — terminate this process and return to the shell. Any extra
    /// words on the line are ignored. Does not unload any driver: exiting
    /// only terminates this REPL's own task, and loaded drivers are
    /// independent, unrelated tasks that keep running.
    Exit,
    /// Anything else, including a recognized command name typed in the
    /// wrong case (dispatch is case-sensitive, matching the shell). Carries
    /// the first whitespace-separated word of the input line.
    Unknown(&'a str),
}

/// Parses one REPL input line into a [`Command`]. Pure and side-effect
/// free: no I/O, no syscalls, so it can be exercised in a plain host test.
fn parse_command(line: &str) -> Command<'_> {
    let line = line.trim();
    if line.is_empty() {
        return Command::Empty;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts
        .next()
        .expect("line was already checked non-empty above");

    match cmd {
        "help" => Command::Help,
        "list" => Command::List,
        "load" => Command::Load(parts.next()),
        "unload" => Command::Unload(parts.next()),
        "exit" => Command::Exit,
        other => Command::Unknown(other),
    }
}

/// Renders the drivers app's welcome banner on startup.
#[cfg(not(test))]
fn print_welcome_banner() {
    println!("========================================");
    println!("    KAOS - Driver Management (DRIVERS.BIN)");
    println!("========================================");
    println!("Type 'help' to see the list of commands.\n");
}

/// Prints the `help` command's output.
#[cfg(not(test))]
fn print_help() {
    println!("Commands:");
    println!("  help              - show this help menu");
    println!("  list              - list currently loaded drivers");
    println!("  load <name.drv>   - load a driver from the filesystem");
    println!("  unload <name>     - unload a currently loaded driver");
    println!("  exit              - exit this application and return to the shell");
}

/// Prints the `list` command's output: every currently loaded driver's name
/// and task id, or a note that none are loaded.
#[cfg(not(test))]
fn print_driver_list() {
    let count = match lib_driver::drv::driver_count() {
        Ok(count) => count,
        Err(err) => {
            println!("list failed: {:?}", err);
            return;
        }
    };

    if count == 0 {
        println!("No drivers loaded.");
        return;
    }

    let empty_entry = lib_driver::UserDriverInfo {
        name: [0u8; lib_driver::USER_DRIVER_NAME_LEN],
        name_len: 0,
        _padding: 0,
        tid: 0,
    };
    let mut entries = alloc::vec![empty_entry; count];
    let filled = match lib_driver::drv::list_drivers(&mut entries) {
        Ok(filled) => filled,
        Err(err) => {
            println!("list failed: {:?}", err);
            return;
        }
    };

    println!("Loaded drivers:");
    for info in &entries[..filled] {
        let name_bytes = &info.name[..(info.name_len as usize).min(info.name.len())];
        match core::str::from_utf8(name_bytes) {
            Ok(name) => println!("  {:<20} tid={}", name, info.tid),
            Err(_) => println!("  <invalid name>       tid={}", info.tid),
        }
    }
}

/// Terminates a loaded driver by name.
///
/// `unload` is a hard kill: the kernel's `DrvUnload` syscall calls
/// `scheduler::terminate_task` directly, which does not run the driver's
/// own shutdown path (no `drop(device)`, no disabling DMA/bus-mastering —
/// see `docs/drivers.md` §15, and `kernel/src/syscall/dispatch/driver.rs`'s
/// `syscall_drv_unload_impl` doc comment). This is a known limitation of
/// this kernel's driver model (no IOMMU), not something this command works
/// around.
#[cfg(not(test))]
fn unload_driver(name: &str) {
    match lib_driver::drv::unload_driver(name.as_bytes()) {
        Ok(()) => println!("[drivers] Driver '{}' unloaded.", name),
        Err(lib_driver::SysError::PermissionDenied) => {
            println!(
                "[drivers] Not authorized to unload '{}' (missing UNLOAD_DRIVER capability).",
                name
            );
        }
        Err(lib_driver::SysError::InvalidArgument) => {
            println!(
                "[drivers] '{}' is not a loaded driver (or the name is invalid).",
                name
            );
        }
        Err(err) => println!("[drivers] Failed to unload '{}': {:?}", name, err),
    }
}

/// Parses and dispatches one entered `drivers` REPL command line.
#[cfg(not(test))]
fn execute_command(line: &str) {
    match parse_command(line) {
        Command::Empty => {}
        Command::Help => print_help(),
        Command::List => print_driver_list(),
        Command::Load(Some(file)) => load_driver::load_driver(file),
        Command::Load(None) => println!("Usage: load <name.drv>"),
        Command::Unload(Some(name)) => unload_driver(name),
        Command::Unload(None) => println!("Usage: unload <name>"),
        Command::Exit => process::exit(),
        Command::Unknown(cmd) => {
            println!("Unknown command: '{}'. Type 'help' for options.", cmd);
        }
    }
}

/// The main entry point of the `drivers` user-space REPL application.
#[cfg(not(test))]
#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    print_welcome_banner();

    let mut buf = [0u8; 128];
    loop {
        print!("drivers> ");
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    println!("\ndrivers Panic: {}", _info);
    process::exit()
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/main.rs"]
mod tests;
