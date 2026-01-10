use crate::drivers::screen::{Color, Screen};
use core::fmt::Write;

/// Physical address of the BIOS information block
pub const BIB_OFFSET: usize = 0x1000;

/// Physical address of the BIOS memory map array
pub const MEMORYMAP_OFFSET: usize = 0x1200;

/// Size of a single page frame in bytes
pub const PAGE_SIZE: u64 = 4096;

#[repr(C)]
/// BIOS-provided system information block shared with the kernel
pub struct BiosInformationBlock {
    /// RTC year
    pub year: i32,

    /// RTC month
    pub month: i16,

    /// RTC day
    pub day: i16,

    /// RTC hour
    pub hour: i16,

    /// RTC minute
    pub minute: i16,

    /// RTC second
    pub second: i16,

    /// Number of entries in the BIOS memory map
    pub memory_map_entries: i16,

    /// Total size of all available memory regions in bytes
    pub max_memory: i64,

    /// Total number of available page frames
    pub available_page_frames: i64
}

#[repr(C)]
/// A single BIOS memory map entry describing a region
pub struct BiosMemoryRegion {
    /// Physical start address of the region
    pub start: u64,

    /// Size of the region in bytes
    pub size: u64,

    /// Region type (1 = usable, others reserved/ACPI/etc.)
    pub region_type: u32,
}

impl BiosInformationBlock {
    /// Prints the memory map that we have obtained from the BIOS in x16 Real Mode.
    pub fn print_memory_map(screen: &mut Screen) {
        // Mutable view so we can mirror the C code: compute MaxMemory and AvailablePageFrames here.
        let bib = unsafe { &mut *(BIB_OFFSET as *mut BiosInformationBlock) };

        let region = MEMORYMAP_OFFSET as *const BiosMemoryRegion;

        let entry_count = bib.memory_map_entries as usize;

        // Reset and recompute MaxMemory / AvailablePageFrames like the C physical memory manager.
        bib.max_memory = 0;
        bib.available_page_frames = 0;

        // Print header
        writeln!(screen, "{} Memory Map entries found.", entry_count).unwrap();

        // Loop over each entry
        for i in 0..entry_count {
            let current_region = unsafe { &*region.add(i) };

            // Set color based on region type
            if current_region.region_type == 1 {
                // Available
                screen.set_color(Color::LightGreen);

                // Track totals for available regions (matches C code behavior)
                bib.max_memory = bib.max_memory.wrapping_add(current_region.size as i64);
                bib.available_page_frames = bib
                    .available_page_frames
                    .wrapping_add((current_region.size / PAGE_SIZE) as i64);
            } else {
                // Everything else
                screen.set_color(Color::LightRed);
            }

            // Start address
            write!(screen, "0x{:010x}", current_region.start).unwrap();

            // End address (use wrapping arithmetic to avoid overflow)
            let end_addr = current_region.start.wrapping_add(current_region.size).wrapping_sub(1);
            write!(screen, " - 0x{:010x}", end_addr).unwrap();

            // Size in hex
            write!(screen, " Size: 0x{:09x}", current_region.size).unwrap();

            // Size in KB
            write!(screen, " {} KB", current_region.size / 1024).unwrap();

            // Size in MB if applicable
            if current_region.size > 1024 * 1024 {
                write!(screen, " = {} MB", current_region.size / 1024 / 1024).unwrap();
            }

            // Memory region type
            let region_type_str = match current_region.region_type {
                1 => "Available",
                2 => "Reserved",
                3 => "ACPI Reclaim",
                4 => "ACPI NVS Memory",
                _ => "Unknown",
            };
            writeln!(screen, " ({})", region_type_str).unwrap();
        }

        // Reset color to white
        screen.set_color(Color::White);

        // Max memory
        writeln!(screen, "Max Memory: {} MB", bib.max_memory / 1024 / 1024 + 1).unwrap();
    }
}
