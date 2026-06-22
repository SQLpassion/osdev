//! Global Time Manager holding state for high-precision system time.

use super::calibration::{calibrate_tsc, rdtsc};
use super::types::DateTime;
use crate::memory::bios::{BiosInformationBlock, BIB_OFFSET};
use crate::sync::spinlock::SpinLock;

/// Structure keeping track of base start time and calibration factors.
pub struct TimeManager {
    boot_time: DateTime,
    boot_tsc: u64,
    tsc_ticks_per_us: u64,
}

static TIME_MANAGER: SpinLock<Option<TimeManager>> = SpinLock::new(None);

/// Initializes the global Time Manager by reading the boot time and calibrating the TSC.
pub fn init() {
    // Step 1: Read the boot time from BIOS Information Block (BIB).
    // SAFETY:
    // - `BIB_OFFSET` is mapped and contains the BIOS Information Block populated by the bootloader.
    // - The memory range is read-only for calibration purposes.
    let bib = unsafe { &*(BIB_OFFSET as *const BiosInformationBlock) };

    // Convert BiosInformationBlock date fields. Since BIB fields can be 0 or uninitialized,
    // we sanitize them to valid calendar bounds.
    let boot_time = DateTime {
        year: bib.year,
        month: if bib.month >= 1 && bib.month <= 12 {
            bib.month as u8
        } else {
            1
        },
        day: if bib.day >= 1 && bib.day <= 31 {
            bib.day as u8
        } else {
            1
        },
        hour: if bib.hour >= 0 && bib.hour < 24 {
            bib.hour as u8
        } else {
            0
        },
        minute: if bib.minute >= 0 && bib.minute < 60 {
            bib.minute as u8
        } else {
            0
        },
        second: if bib.second >= 0 && bib.second < 60 {
            bib.second as u8
        } else {
            0
        },
    };

    // Step 2: Calibrate the TSC.
    let ticks_per_us = calibrate_tsc();
    let start_tsc = rdtsc();

    // Step 3: Initialize the global state.
    let mut lock = TIME_MANAGER.lock();
    *lock = Some(TimeManager {
        boot_time,
        boot_tsc: start_tsc,
        tsc_ticks_per_us: ticks_per_us,
    });
}

/// Returns the current DateTime by extrapolating the elapsed TSC cycles since boot.
pub fn get_time() -> DateTime {
    let lock = TIME_MANAGER.lock();
    let manager = match &*lock {
        Some(m) => m,
        None => {
            // Fallback if not initialized
            return DateTime {
                year: 2026,
                month: 6,
                day: 8,
                hour: 0,
                minute: 0,
                second: 0,
            };
        }
    };

    let current_tsc = rdtsc();
    let elapsed_cycles = current_tsc.saturating_sub(manager.boot_tsc);

    // Convert cycles to seconds: cycles / (ticks_per_us * 1,000,000)
    let divisor = manager.tsc_ticks_per_us.saturating_mul(1_000_000);

    // Step 1: Safely divide cycles by the frequency divisor to avoid division-by-zero.
    let elapsed_seconds = elapsed_cycles.checked_div(divisor).unwrap_or(0);

    let mut current_time = manager.boot_time;
    current_time.add_seconds(elapsed_seconds);
    current_time
}
