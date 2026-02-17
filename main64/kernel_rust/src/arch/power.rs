//! Power control helpers (best-effort)
//!
//! For QEMU/Bochs this attempts ACPI S5 poweroff by writing to well-known
//! PM control ports. On real hardware a proper ACPI parser is needed to
//! locate PM1 control blocks and SLP_TYP. If this path fails, we halt.

use crate::arch::port::PortWord;
use core::arch::asm;

/// Attempt to power off. Works on QEMU/Bochs; halts otherwise.
pub fn shutdown() -> ! {
    unsafe {
        // QEMU/Bochs ACPI S5: write 0x2000 to 0x604 and 0xB004.
        // If unsupported, execution continues to the halt loop.
        let pm1 = PortWord::new(0x604);
        pm1.write(0x2000);
        let pm1b = PortWord::new(0xB004);
        pm1b.write(0x2000);

        // Fallback: stop the CPU.
        loop {
            asm!("hlt");
        }
    }
}
