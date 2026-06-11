//! Calibration logic for the CPU Time Stamp Counter (TSC).

use crate::arch::port::PortByte;

/// Reads the current value of the Time Stamp Counter.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY:
    // - `rdtsc` is a safe, non-privileged instruction to read the CPU timestamp counter.
    // - Register inputs/outputs are correctly mapped.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Calibrates the TSC against PIT Channel 2 over a ~10 millisecond window.
///
/// Returns the number of TSC cycles per microsecond.
pub fn calibrate_tsc() -> u64 {
    let cmd = PortByte::new(0x43);
    let chan2 = PortByte::new(0x42);
    let gate = PortByte::new(0x61);

    // SAFETY:
    // - Disabling the PIT Channel 2 gate is safe and only modifies the PIT2 configuration.
    let gate_orig = unsafe { gate.read() };
    // SAFETY:
    // - Writing to port 0x61 to disable PIT2 gate.
    unsafe { gate.write(gate_orig & !0x01); }

    // Program PIT Channel 2: Mode 0 (Interrupt on terminal count), LOBYTE/HIBYTE, binary.
    // Command: 0b10110000 = 0xB0.
    // SAFETY:
    // - Programming the PIT control word is safe inside ring 0.
    unsafe { cmd.write(0xB0); }

    // Divisor for 10 ms delay: 1193182 Hz / 100 = 11931 = 0x2E9B.
    // SAFETY:
    // - Writing PIT2 divisor bytes is safe inside ring 0.
    unsafe {
        chan2.write(0x9B);
        chan2.write(0x2E);
    }

    // Enable PIT Channel 2 gate (bit 0) without speaker (bit 1).
    // SAFETY:
    // - Writing to port 0x61 is safe to start the timer.
    unsafe { gate.write((gate.read() & !0x02) | 0x01); }

    let start_tsc = rdtsc();

    // Loop until PIT Channel 2 count reaches 0 or wraps.
    loop {
        // Latch Channel 2 count: write 0b10000000 (0x80) to command port 0x43.
        // SAFETY:
        // - Latching PIT2 count is safe inside ring 0.
        unsafe { cmd.write(0x80); }
        // SAFETY:
        // - Reading latch values from PIT2 data port 0x42 is safe.
        let lo = unsafe { chan2.read() };
        let hi = unsafe { chan2.read() };
        let count = ((hi as u16) << 8) | (lo as u16);
        if count == 0 || count > 11931 {
            break;
        }
    }

    let end_tsc = rdtsc();

    // Restore original gate settings.
    // SAFETY:
    // - Restoring original gate state on port 0x61 is safe.
    unsafe { gate.write(gate_orig); }

    let diff = end_tsc.saturating_sub(start_tsc);
    
    // 10 milliseconds = 10,000 microseconds.
    let cycles_per_us = diff / 10000;
    if cycles_per_us == 0 {
        // Fallback value (e.g. 2.0 GHz) if calibration failed or emulator behaves unexpectedly.
        2000
    } else {
        cycles_per_us
    }
}
