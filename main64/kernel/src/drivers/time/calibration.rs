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

/// Maximum number of PIT polling iterations before TSC calibration times out.
///
/// If Channel 2 never counts (gate misbehavior or a broken PIT), the loop
/// aborts and returns the default frequency instead of hanging forever.
const TSC_CALIBRATION_TIMEOUT_ITERATIONS: u32 = 1_000_000;

/// Minimum plausible TSC delta for a 10 ms calibration window.
///
/// Corresponds to 1 cycle per microsecond (1 MHz TSC). Anything smaller
/// indicates the PIT count was sampled before the counter loaded or the
/// calibration window was otherwise invalid.
const TSC_CALIBRATION_MIN_DIFF: u64 = 10_000;

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
    unsafe {
        gate.write(gate_orig & !0x01);
    }

    // Program PIT Channel 2: Mode 0 (Interrupt on terminal count), LOBYTE/HIBYTE, binary.
    // Command: 0b10110000 = 0xB0.
    // SAFETY:
    // - Programming the PIT control word is safe inside ring 0.
    unsafe {
        cmd.write(0xB0);
    }

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
    unsafe {
        gate.write((gate.read() & !0x02) | 0x01);
    }

    let start_tsc = rdtsc();

    // Loop until PIT Channel 2 count reaches 0 or wraps.
    let mut timeout = TSC_CALIBRATION_TIMEOUT_ITERATIONS;
    loop {
        // Latch Channel 2 count: write 0b10000000 (0x80) to command port 0x43.
        // SAFETY:
        // - Latching PIT2 count is safe inside ring 0.
        unsafe {
            cmd.write(0x80);
        }
        // SAFETY:
        // - Reading latch values from PIT2 data port 0x42 is safe.
        let lo = unsafe { chan2.read() };
        let hi = unsafe { chan2.read() };
        let count = ((hi as u16) << 8) | (lo as u16);
        if count == 0 || count > 11931 {
            break;
        }

        // Abort if the PIT counter never progresses, avoiding an infinite
        // boot hang on misbehaving hardware or emulators.
        if timeout == 0 {
            // Restore original gate settings before returning the fallback.
            // SAFETY:
            // - Restoring original gate state on port 0x61 is safe.
            unsafe {
                gate.write(gate_orig);
            }
            return 2000;
        }
        timeout -= 1;
    }

    let end_tsc = rdtsc();

    // Restore original gate settings.
    // SAFETY:
    // - Restoring original gate state on port 0x61 is safe.
    unsafe {
        gate.write(gate_orig);
    }

    let diff = end_tsc.saturating_sub(start_tsc);

    // Reject implausibly small deltas (e.g. the first latch happened before
    // the counter loaded) and fall back to a safe default.
    if diff < TSC_CALIBRATION_MIN_DIFF {
        return 2000;
    }

    // 10 milliseconds = 10,000 microseconds.
    diff / 10000
}
