//! QEMU Debug Exit Device
//!
//! This module provides functionality to exit QEMU with a specific exit code.
//! Used by the test framework to signal test success or failure to the host.
//!
//! QEMU must be started with: -device isa-debug-exit,iobase=0xf4,iosize=0x04
//!
//! The exit code written to port 0xF4 is transformed by QEMU:
//! actual_exit_code = (value << 1) | 1
//!
//! So writing 0x10 results in exit code 33 (success)
//! And writing 0x11 results in exit code 35 (failure)

use crate::arch::port::PortByte;

/// QEMU debug exit device I/O port
#[allow(dead_code)]
const QEMU_EXIT_PORT: u16 = 0xF4;

/// Exit codes for QEMU (these get transformed by QEMU)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
#[repr(u8)]
pub enum QemuExitCode {
    /// Success - QEMU will exit with code 33 ((0x10 << 1) | 1)
    Success = 0x10,
    /// Failure - QEMU will exit with code 35 ((0x11 << 1) | 1)
    Failed = 0x11,
}

/// Exit QEMU with the specified exit code
///
/// This function writes to the QEMU debug exit device port.
/// QEMU must be started with: -device isa-debug-exit,iobase=0xf4,iosize=0x04
///
/// # Note
/// This function only works when running under QEMU with the debug exit device.
/// On real hardware, this will have no effect (or undefined behavior).
#[allow(dead_code)]
pub fn exit_qemu(exit_code: QemuExitCode) -> ! {
    unsafe {
        let port = PortByte::new(QEMU_EXIT_PORT);
        port.write(exit_code as u8);
    }

    // If we're still running (e.g., not in QEMU), halt
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
