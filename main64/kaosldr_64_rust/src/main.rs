#![no_std]
#![no_main]
#![allow(clippy::empty_loop)]

use core::fmt::Write;
use core::panic::PanicInfo;

mod asm;
mod ata;
mod fat12;
mod vga;

use asm::execute_kernel;
use fat12::load_kernel_into_memory;
use vga::VgaWriter;

/// Entry point of KLDR64.BIN
/// The only purpose of the KLDR64.BIN file is to load the KERNEL.BIN file to the physical
/// memory address 0x100000 and execute it from there.
///
/// This task must be done in KLDR64.BIN, because the CPU is now already in x64 Long Mode,
/// and therefore we can access higher memory addresses like 0x100000.
/// This would be impossible to do in KLDR16.BIN, because the CPU is at that point in time still in x16 Real Mode.
///
/// # Safety
/// This function is called from the 16-bit to 64-bit assembly transition loader
/// and must never return. It runs in ring 0 long mode.
#[no_mangle]
#[link_section = ".text.boot"]
pub unsafe extern "C" fn kaosldr_main() -> ! {
    let mut writer = VgaWriter::new();
    writer.clear_screen();

    // Load the x64 OS Kernel into memory for its execution...
    // The filename must be padded to 11 characters ("KERNEL  BIN")
    match load_kernel_into_memory(b"KERNEL  BIN") {
        Ok(sectors) => {
            let kernel_size = sectors * 512;

            // Execute the Kernel.
            // This function call will never return...
            execute_kernel(kernel_size);
        }
        Err(msg) => {
            let _ = writer.write_str("Error: ");
            let _ = writer.write_str(msg);
            let _ = writer.write_str("\n");
        }
    }

    // Safety fallback loop
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let mut writer = VgaWriter::new();
    let _ = writer.write_str("\nPANIC in kaosldr_64\n");
    loop {}
}
