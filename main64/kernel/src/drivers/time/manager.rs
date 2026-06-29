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
    // Step 1: Query the global BootInfo pointer. If it has been populated by the bootloader
    // (UEFI or modern BIOS path), we read the time fields directly from the BootInfo structure.
    let boot_info_raw = crate::boot_info::BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Acquire);

    let boot_time = if boot_info_raw != 0 {
        // SAFETY:
        // - `boot_info_raw` was validated by the kernel entry point to ensure it contains a valid
        //   and aligned physical address pointing to the unified `BootInfo` structure.
        // - The memory range is mapped and valid for read access during early boot.
        // - There are no concurrent writers, making read access safe.
        // - If `boot_info_raw` were null or invalid, dereferencing would trigger a page fault.
        let bi = unsafe { &*(boot_info_raw as *const crate::boot_info::BootInfo) };
        DateTime {
            year: bi.boot_year as i32,
            month: if bi.boot_month >= 1 && bi.boot_month <= 12 {
                bi.boot_month
            } else {
                1
            },
            day: if bi.boot_day >= 1 && bi.boot_day <= 31 {
                bi.boot_day
            } else {
                1
            },
            hour: if bi.boot_hour < 24 { bi.boot_hour } else { 0 },
            minute: if bi.boot_minute < 60 {
                bi.boot_minute
            } else {
                0
            },
            second: if bi.boot_second < 60 {
                bi.boot_second
            } else {
                0
            },
        }
    } else {
        // Step 2: Fallback path. If no BootInfo is available (e.g. legacy BIOS tests),
        // we read the boot time from the BIOS Information Block (BIB) at physical address 0x1000.
        // SAFETY:
        // - `BIB_OFFSET` is mapped and contains the BiosInformationBlock populated by the bootloader.
        // - The memory range is read-only for calibration purposes.
        // - If the bootloader failed to map or write this block, it would cause a CPU exception.
        let bib = unsafe { &*(BIB_OFFSET as *const BiosInformationBlock) };
        DateTime {
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
        }
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
