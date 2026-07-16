//! Interactive Ring-3 exception exerciser.
//!
//! This program is intentionally launched as a child from `SHELL.BIN`:
//! `exec except.bin`. A successfully recovered exception terminates only this
//! process, allowing the parent shell to demonstrate that it survived.

#![no_std]
#![no_main]

use lib_kaos::{console, println, process};

/// Fault scenarios supported by the interactive menu.
///
/// The variants are retained even while the corresponding kernel recovery path
/// is not enabled, so adding a future exception handler only requires enabling
/// the trigger in `run_selected_fault`.
#[derive(Clone, Copy)]
enum FaultKind {
    InvalidOpcode,
    DivideError,
    GeneralProtection,
    UnmappedPage,
    KernelPage,
    NoExecutePage,
}

/// Draws the initial selection screen and documents current safety status.
fn show_welcome_screen() {
    let _ = console::clear_screen();
    println!("================================================");
    println!("        KAOS Ring-3 Exception Exerciser");
    println!("================================================");
    println!("This program deliberately faults only itself.");
    println!("Launch it from SHELL.BIN: exec except.bin\n");
    println!("Available now:");
    println!("  [U] Invalid opcode (#UD) - terminates this task");
    println!("\nReserved until their kernel recovery paths exist:");
    println!("  [D] Divide error (#DE)");
    println!("  [G] General protection (#GP)");
    println!("  [P] Unmapped user page (#PF)");
    println!("  [K] Kernel page access (#PF protection)");
    println!("  [X] Execute an NX page (#PF instruction fetch)");
    println!("\n  [Q] Exit without triggering an exception");
    println!("\nSelect a test key:");
}

/// Maps one key event to the requested diagnostic scenario.
fn fault_for_key(key: console::Key) -> Option<FaultKind> {
    match key {
        console::Key::Char(b'u' | b'U') => Some(FaultKind::InvalidOpcode),
        console::Key::Char(b'd' | b'D') => Some(FaultKind::DivideError),
        console::Key::Char(b'g' | b'G') => Some(FaultKind::GeneralProtection),
        console::Key::Char(b'p' | b'P') => Some(FaultKind::UnmappedPage),
        console::Key::Char(b'k' | b'K') => Some(FaultKind::KernelPage),
        console::Key::Char(b'x' | b'X') => Some(FaultKind::NoExecutePage),
        _ => None,
    }
}

/// Executes a selected fault only when the kernel has a corresponding recovery path.
fn run_selected_fault(fault: FaultKind) {
    match fault {
        FaultKind::InvalidOpcode => {
            println!("Triggering #UD. The shell should return after this task exits.");
            trigger_invalid_opcode();
        }
        FaultKind::DivideError => unavailable("#DE", "divide-error"),
        FaultKind::GeneralProtection => unavailable("#GP", "general-protection"),
        FaultKind::UnmappedPage => unavailable("#PF", "unmapped-page"),
        FaultKind::KernelPage => unavailable("#PF", "kernel-page protection"),
        FaultKind::NoExecutePage => unavailable("#PF", "NX instruction-fetch"),
    }
}

/// Explains why an advertised future case is deliberately not triggered yet.
fn unavailable(vector: &str, scenario: &str) {
    println!(
        "{} ({}) recovery is not enabled in this kernel yet.",
        vector, scenario
    );
    println!("No exception was triggered; choose another test key.");
}

/// Raises the architecturally defined Invalid Opcode exception.
fn trigger_invalid_opcode() -> ! {
    // SAFETY:
    // - `ud2` is deliberately executed in Ring 3 to raise #UD (vector 6).
    // - The instruction never retires; the kernel #UD handler terminates this task.
    // - If the handler incorrectly resumed this task, re-executing `ud2` is
    //   still preferable to continuing with an invalid diagnostic state.
    unsafe {
        core::arch::asm!("ud2", options(noreturn));
    }
}

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    show_welcome_screen();

    loop {
        match console::read_key() {
            Ok(console::Key::Char(b'q' | b'Q')) | Ok(console::Key::Escape) => {
                println!("Leaving exception exerciser.");
                process::exit();
            }
            Ok(key) => match fault_for_key(key) {
                Some(fault) => run_selected_fault(fault),
                None => println!("Unknown selection. Use U, D, G, P, K, X, or Q."),
            },
            Err(error) => println!("Could not read a key: {:#x}", error),
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
