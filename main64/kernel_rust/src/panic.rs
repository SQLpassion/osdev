//! Panic handler for the kernel
//!
//! Required for `no_std` environments.

use crate::drivers::screen::Color;
use core::fmt::Write;
use core::panic::PanicInfo;

/// Panic handler - called when the kernel panics
///
/// In Phase 1, we just print an error message and halt.
/// Later phases will add stack traces and debugging info.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Step 1: render panic text through a lock-free VGA writer.
    //
    // Why lock-free:
    // - panic may occur while `GLOBAL_SCREEN` lock is already held.
    // - taking the same lock again would deadlock the panic path.
    let mut screen = crate::drivers::screen::PanicScreenWriter::new(Color::White, Color::Blue);
    screen.clear();

    let _ = writeln!(screen, "!!! KERNEL PANIC !!!");

    if let Some(location) = info.location() {
        let _ = writeln!(screen, "Location: {}:{}", location.file(), location.line());
        let _ = writeln!(screen);
    }

    let _ = writeln!(screen, "Message: {}", info.message());

    // Halt the CPU
    loop {
        // SAFETY:
        // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
        // - Panic path intentionally stops all forward progress.
        // - `cli`/`hlt` are privileged instructions and valid in ring 0.
        unsafe {
            core::arch::asm!("cli"); // Disable interrupts
            core::arch::asm!("hlt"); // Halt
        }
    }
}
