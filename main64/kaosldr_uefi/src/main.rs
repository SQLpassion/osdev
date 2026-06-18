#![no_std]
#![no_main]
#![allow(clippy::empty_loop)]

//! KAOS UEFI Loader (`BOOTX64.EFI`)
//!
//! This is the UEFI counterpart to the legacy boot chain (`bootsector.asm` -> `kaosldr_16`
//! -> `kaosldr_64`). The firmware loads this PE32+ executable from `/EFI/BOOT/BOOTX64.EFI`
//! on the EFI System Partition and calls [`efi_main`] while still in 64-bit long mode with
//! the UEFI Boot Services available.
//!
//! Current scope (first milestone): prove that the toolchain works end to end by printing a
//! message via the UEFI Simple Text Output Protocol (`ConOut`). No kernel is loaded yet, the
//! framebuffer is not touched, and Boot Services are not exited. Subsequent milestones will
//! query the GOP framebuffer, load `KERNEL.BIN` into RAM, call `ExitBootServices()` and jump
//! to the kernel.
//!
//! The minimal subset of the UEFI structures required for this is declared by hand below, so
//! the loader stays free of external dependencies.

use core::ffi::c_void;
use core::panic::PanicInfo;

mod serial;

/// UEFI status code (`EFI_STATUS`, a `UINTN`). `0` is `EFI_SUCCESS`.
pub type Status = usize;

/// UEFI handle (`EFI_HANDLE`), an opaque pointer.
pub type Handle = *mut c_void;

/// Header common to all UEFI tables (`EFI_TABLE_HEADER`).
#[repr(C)]
struct EfiTableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}

/// The UEFI Simple Text Output Protocol (`EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`).
///
/// Only the first two function pointers are declared; the remaining members
/// (`TestString`, `QueryMode`, `SetMode`, ...) are not needed for this milestone, so the
/// struct intentionally describes only the leading prefix of the real protocol.
#[repr(C)]
struct EfiSimpleTextOutputProtocol {
    /// Resets the output device. Unused here.
    reset: extern "efiapi" fn(this: *mut EfiSimpleTextOutputProtocol, extended: bool) -> Status,
    /// Writes a NUL-terminated UCS-2 string to the console.
    output_string:
        extern "efiapi" fn(this: *mut EfiSimpleTextOutputProtocol, string: *const u16) -> Status,
}

/// The UEFI System Table (`EFI_SYSTEM_TABLE`).
///
/// Declared only up to and including the `con_out` pointer; everything after it (runtime
/// services, boot services, configuration table, ...) is omitted because this milestone does
/// not use it. `#[repr(C)]` reproduces the exact field offsets of the real table, including
/// the padding the firmware inserts after `firmware_revision`.
#[repr(C)]
pub struct EfiSystemTable {
    hdr: EfiTableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: Handle,
    con_in: *mut c_void,
    console_out_handle: Handle,
    con_out: *mut EfiSimpleTextOutputProtocol,
}

/// Entry point of the UEFI application.
///
/// The `x86_64-unknown-uefi` target fixes the PE entry point to the `efi_main` symbol and the
/// `efiapi` calling convention, so this signature must match what the firmware expects.
///
/// # Safety
/// `system_table` is provided by the firmware and is assumed to be a valid pointer to an
/// `EFI_SYSTEM_TABLE` with a working `ConOut` protocol, as guaranteed by the UEFI spec for a
/// loaded application. The function never exits Boot Services and never returns control to
/// the firmware (it halts in an idle loop).
#[no_mangle]
pub extern "efiapi" fn efi_main(_image_handle: Handle, system_table: *const EfiSystemTable) -> Status {
    // Bring up the serial port first so the messages are visible even on a headless host.
    serial::init();

    // SAFETY: The firmware passes a valid, non-null system table to a loaded application.
    // We only read the `con_out` pointer from it.
    let con_out = unsafe { (*system_table).con_out };

    print(con_out, "KAOS UEFI loader: hello from BOOTX64.EFI\r\n");
    print(con_out, "Toolchain OK - running in long mode under UEFI Boot Services.\r\n");

    // Do not exit Boot Services yet and do not return; idle so the message stays on screen.
    loop {}
}

/// Writes an ASCII string to the UEFI console via `ConOut->OutputString`.
///
/// UEFI expects NUL-terminated UCS-2 (`CHAR16`) strings. Since the message is plain ASCII,
/// each byte is widened to a `u16`. The string is emitted in chunks through a small stack
/// buffer, so arbitrarily long messages work without a heap.
fn print(con_out: *mut EfiSimpleTextOutputProtocol, s: &str) {
    // Mirror everything to COM1 so the output is visible without a display.
    serial::write_str(s);

    /// Number of UCS-2 code units buffered per firmware call (plus one slot for the
    /// terminating NUL).
    const CHUNK: usize = 64;
    let mut buffer = [0u16; CHUNK + 1];
    let mut len = 0;

    for byte in s.bytes() {
        buffer[len] = byte as u16;
        len += 1;

        if len == CHUNK {
            buffer[len] = 0;
            flush(con_out, &buffer);
            len = 0;
        }
    }

    if len > 0 {
        buffer[len] = 0;
        flush(con_out, &buffer);
    }
}

/// Hands a single NUL-terminated UCS-2 buffer to the firmware.
fn flush(con_out: *mut EfiSimpleTextOutputProtocol, buffer: &[u16]) {
    // SAFETY: `con_out` is a valid protocol pointer obtained from the firmware-provided
    // system table, and `buffer` is NUL-terminated UCS-2 as required by OutputString.
    unsafe {
        ((*con_out).output_string)(con_out, buffer.as_ptr());
    }
}

/// Panic handler: there is nothing to unwind to under UEFI, so halt forever.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
