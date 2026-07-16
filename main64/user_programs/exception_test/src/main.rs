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
    println!("  [P] Unmapped user page (#PF) - terminates this task");
    println!("  [D] Divide error (#DE) - terminates this task");
    println!("  [G] General protection (#GP) - terminates this task");
    println!("  [K] Kernel page access (#PF protection) - terminates this task");
    println!("  [X] Execute an NX page (#PF instruction fetch) - terminates this task");
    println!("\nAll listed exception scenarios are active:");
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
        FaultKind::DivideError => {
            println!("Triggering #DE. The shell should return after this task exits.");
            trigger_divide_error();
        }
        FaultKind::GeneralProtection => {
            println!("Triggering #GP. The shell should return after this task exits.");
            trigger_general_protection();
        }
        FaultKind::UnmappedPage => {
            println!("Triggering #PF. The shell should return after this task exits.");
            trigger_unmapped_page_fault();
        }
        FaultKind::KernelPage => {
            println!("Triggering kernel-page #PF. The shell should return after this task exits.");
            trigger_kernel_page_fault();
        }
        FaultKind::NoExecutePage => {
            println!("Triggering NX #PF. The shell should return after this task exits.");
            trigger_no_execute_page_fault();
        }
    }
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

/// Raises the architecturally defined Divide Error exception.
fn trigger_divide_error() -> ! {
    // SAFETY:
    // - Clearing the dividend and divisor deliberately creates a zero divisor.
    // - The instruction never retires; the kernel #DE handler terminates this task.
    unsafe {
        core::arch::asm!("xor edx, edx", "xor eax, eax", "div edx", options(noreturn));
    }
}

/// Raises a General Protection Fault by executing the privileged `WRMSR` instruction.
fn trigger_general_protection() -> ! {
    // SAFETY:
    // - `wrmsr` is privileged and therefore raises #GP when executed in Ring 3.
    // - The zeroed MSR number is immaterial because privilege checking happens
    //   before the requested MSR is accessed.
    // - The instruction never completes; the kernel #GP handler terminates this task.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") 0u32,
            in("eax") 0u32,
            in("edx") 0u32,
            options(noreturn),
        );
    }
}

/// Reads an unmapped user address to raise a non-present Ring-3 page fault.
fn trigger_unmapped_page_fault() -> ! {
    const UNMAPPED_USER_ADDRESS: usize = 0x0000_6000_0000_0000;

    // SAFETY:
    // - The address lies outside every user mapping window configured by the kernel.
    // - A volatile read ensures the compiler emits the access that raises #PF.
    // - The access never completes because the kernel terminates this task.
    unsafe {
        core::ptr::read_volatile(UNMAPPED_USER_ADDRESS as *const u8);
    }

    loop {
        core::hint::spin_loop();
    }
}

/// Raises a protection page fault by reading a supervisor-only kernel mapping.
fn trigger_kernel_page_fault() -> ! {
    const KERNEL_TEXT_ADDRESS: usize = 0xFFFF_8000_0010_0000;

    // SAFETY:
    // - The higher-half kernel mapping is present but supervisor-only.
    // - A Ring-3 volatile read therefore raises #PF with P=1 and U=1.
    // - The faulting access never completes because the kernel terminates this task.
    unsafe {
        core::ptr::read_volatile(KERNEL_TEXT_ADDRESS as *const u8);
    }

    loop {
        core::hint::spin_loop();
    }
}

/// Raises an instruction-fetch page fault on the non-executable user stack.
fn trigger_no_execute_page_fault() -> ! {
    const USER_STACK_TOP: usize = 0x0000_7FFF_F000_0000;
    const NX_STACK_ADDRESS: usize = USER_STACK_TOP - 0x1000 + 0x100;

    // SAFETY:
    // - The loader maps the top user-stack page as present, user, writable, and NX.
    // - Calling this address performs an instruction fetch from that NX page.
    // - The fetch faults before any target instruction can execute.
    let target: extern "C" fn() -> ! = unsafe { core::mem::transmute(NX_STACK_ADDRESS) };
    target();
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
