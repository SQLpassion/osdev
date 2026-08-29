#![no_std]
#![no_main]

extern crate alloc;

mod fpu;

use fpu::run_fpu_smoke_test;
use lib_kaos::{console, fs, print, println, process};

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
            println!("  dir             - list directory contents of the FAT32 disk");
            println!("  cat <file>      - read and print the contents of a file");
            println!("  exec <file>     - run a program in the foreground");
            println!("  fputest         - run FPU/SSE smoke test (ring 3)");
            println!("  except          - launch the Ring-3 exception exerciser");
            println!("  kbasic          - run the BASIC interpreter");
            println!("  rtl8139         - start the RTL8139 network driver");
            println!("  date            - show the current calendar date");
            println!("  time            - show the current system time");
            println!("  exit            - exit this shell instance");
            println!("  shutdown        - shutdown the system");
        }
        "kbasic" => {
            run_program("kbasic.bin");
        }
        "rtl8139" | "rtl8139.bin" => {
            run_rtl8139_driver();
        }
        "driver" => {
            if let Some(target) = parts.next() {
                if target.eq_ignore_ascii_case("rtl8139")
                    || target.eq_ignore_ascii_case("rtl8139.bin")
                {
                    run_rtl8139_driver();
                } else {
                    println!("Unknown driver '{}'", target);
                }
            } else {
                println!("Usage: driver <driver-name>");
            }
        }
        "date" => {
            let mut udt = lib_kaos::time::UserDateTime {
                year: 0,
                month: 0,
                day: 0,
                hour: 0,
                minute: 0,
                second: 0,
                _padding: [0; 7],
            };
            if lib_kaos::time::get_time(&mut udt).is_ok() {
                println!("{:04}-{:02}-{:02}", udt.year, udt.month, udt.day);
            } else {
                println!("Error: Failed to retrieve system date.");
            }
        }
        "time" => {
            let mut udt = lib_kaos::time::UserDateTime {
                year: 0,
                month: 0,
                day: 0,
                hour: 0,
                minute: 0,
                second: 0,
                _padding: [0; 7],
            };
            if lib_kaos::time::get_time(&mut udt).is_ok() {
                println!("{:02}:{:02}:{:02}", udt.hour, udt.minute, udt.second);
            } else {
                println!("Error: Failed to retrieve system time.");
            }
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
                if file_name.eq_ignore_ascii_case("rtl8139.bin")
                    || file_name.eq_ignore_ascii_case("rtl8139")
                {
                    run_rtl8139_driver();
                } else {
                    run_program(file_name);
                }
            } else {
                println!("Usage: exec <8.3-filename>");
            }
        }
        "fputest" => {
            run_fpu_smoke_test();
        }
        "except" => {
            run_program("except.bin");
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
            if other.eq_ignore_ascii_case("rtl8139.bin") {
                run_rtl8139_driver();
            } else {
                run_program(other);
            }
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

/// Spawns the RTL8139 network driver with authorized MMIO and IRQ capabilities.
fn run_rtl8139_driver() {
    use lib_driver::spawn::spawn_driver;
    use lib_driver::UserDriverGrants;
    use lib_kaos::pci;

    println!("Scanning PCI for Realtek RTL8139 network card...");
    let dev_count = pci::get_pci_device_count().unwrap_or(0);
    let mut grants = UserDriverGrants {
        mmio_base: 0,
        mmio_len: 0,
        irq: 0xFF,
        _padding: [0; 7],
    };

    for i in 0..dev_count {
        let mut dev = pci::UserPciDevice {
            bus: 0,
            device: 0,
            function: 0,
            class_code: 0,
            subclass: 0,
            prog_if: 0,
            revision_id: 0,
            header_type: 0,
            vendor_id: 0,
            device_id: 0,
            interrupt_line: 0,
            interrupt_pin: 0,
            _padding: [0; 2],
            bars: [pci::UserPciBar {
                bar_type: 0,
                flags: 0,
                address: 0,
                size: 0,
                raw_value: 0,
                _padding: 0,
            }; 6],
        };

        if pci::get_pci_device(i, &mut dev).is_ok()
            && dev.vendor_id == 0x10EC
            && dev.device_id == 0x8139
        {
            let mut mmio_bar = None;
            for bar in &dev.bars {
                if (bar.bar_type == 2 || bar.bar_type == 3) && bar.address != 0 {
                    mmio_bar = Some(*bar);
                    break;
                }
            }
            let bar = mmio_bar.unwrap_or(if dev.bars[1].address != 0 {
                dev.bars[1]
            } else {
                dev.bars[0]
            });
            grants.mmio_base = bar.address;
            grants.mmio_len = if bar.size != 0 { bar.size } else { 256 };
            grants.irq = dev.interrupt_line;
            break;
        }
    }

    let caps = 1 | 2; // MMIO (1) | IRQ (2)
    let grants_opt = if grants.mmio_len > 0 {
        Some(&grants)
    } else {
        None
    };

    println!("Spawning RTL8139 driver with MMIO + IRQ capabilities...");
    match spawn_driver("rtl8139.bin", caps, grants_opt) {
        Ok(pid) => {
            if let Err(err) = process::wait(pid as usize) {
                println!("Error waiting for RTL8139 driver: error {:#x}", err);
            }
        }
        Err(err) => {
            println!("Failed to spawn RTL8139 driver: {:?}", err);
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
