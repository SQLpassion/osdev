#![no_std]
#![no_main]
#![allow(clippy::empty_loop)]

use core::fmt::Write;
use core::panic::PanicInfo;

mod asm;
mod ata;
mod boot_info;
mod fat12;
mod vga;

use asm::execute_kernel;
use boot_info::{BootInfo, FramebufferInfo, UnifiedMemoryEntry, VideoModeType};
use fat12::load_kernel_into_memory;
use vga::VgaWriter;

/// Physical address of the BIOS information block
const BIB_OFFSET: usize = 0x1000;

/// Physical address of the BIOS memory map array
const MEMORYMAP_OFFSET: usize = 0x1200;

#[repr(C)]
struct BiosInformationBlock {
    year: i32,
    month: i16,
    day: i16,
    hour: i16,
    minute: i16,
    second: i16,
    memory_map_entries: i16,
    max_memory: i64,
    available_page_frames: i64,
    video_type: u32,
    _padding: u32,
    fb_base_address: u64,
    fb_size: u64,
    fb_width: u32,
    fb_height: u32,
    fb_pixels_per_scanline: u32,
    _padding2: u32,
}

#[repr(C)]
struct BiosMemoryRegion {
    start: u64,
    size: u64,
    region_type: u32,
}

/// Static buffer to hold the translated unified memory map.
static mut UNIFIED_MEM_MAP: [UnifiedMemoryEntry; 128] = [UnifiedMemoryEntry {
    start: 0,
    size: 0,
    is_usable: false,
}; 128];

static mut BOOT_INFO: BootInfo = BootInfo {
    magic: 0x4B414F535F424F4F,
    video_type: VideoModeType::VgaText,
    fb_info: FramebufferInfo {
        base_address: 0,
        size: 0,
        width: 0,
        height: 0,
        pixels_per_scanline: 0,
        pixel_format: boot_info::PixelFormat::Bgr,
    },
    memory_map_addr: 0,
    memory_map_len: 0,
    kernel_size: 0,
    pmm_metadata_base: 0,
    pmm_metadata_size: 0,
    boot_year: 0,
    boot_month: 0,
    boot_day: 0,
    boot_hour: 0,
    boot_minute: 0,
    boot_second: 0,
    boot_timezone: 0,
};

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
            let kernel_size = (sectors as u64) * 512;

            // SAFETY:
            // - `BIB_OFFSET` is set by the 16-bit loader in low memory and is valid.
            // - `MEMORYMAP_OFFSET` is set by the 16-bit loader and is valid.
            // - We translate the BIOS E820 memory map to the new unified structure format.
            // - Write raw pointer values into static mutable structures before jumping to the kernel.
            #[allow(clippy::needless_range_loop)]
            unsafe {
                let bib = &*(BIB_OFFSET as *const BiosInformationBlock);
                let region = MEMORYMAP_OFFSET as *const BiosMemoryRegion;
                let entry_count = bib.memory_map_entries as usize;

                for i in 0..entry_count.min(128) {
                    let current_region = &*region.add(i);
                    UNIFIED_MEM_MAP[i] = UnifiedMemoryEntry {
                        start: current_region.start,
                        size: current_region.size,
                        is_usable: current_region.region_type == 1,
                    };
                }

                BOOT_INFO.memory_map_addr = &raw const UNIFIED_MEM_MAP[0] as u64;
                BOOT_INFO.memory_map_len = entry_count.min(128) as u32;
                BOOT_INFO.kernel_size = kernel_size;

                // Translate the selected BIOS video mode and framebuffer properties into the BootInfo block.
                BOOT_INFO.video_type = if bib.video_type == 1 {
                    VideoModeType::Framebuffer
                } else {
                    VideoModeType::VgaText
                };
                BOOT_INFO.fb_info = FramebufferInfo {
                    base_address: bib.fb_base_address,
                    size: bib.fb_size as usize,
                    width: bib.fb_width,
                    height: bib.fb_height,
                    pixels_per_scanline: bib.fb_pixels_per_scanline,
                    pixel_format: boot_info::PixelFormat::Bgr,
                };

                // Copy date and time from the legacy BIOS Information Block (BIB)
                BOOT_INFO.boot_year = bib.year.max(0) as u16;
                BOOT_INFO.boot_month = if bib.month >= 1 && bib.month <= 12 {
                    bib.month as u8
                } else {
                    1
                };
                BOOT_INFO.boot_day = if bib.day >= 1 && bib.day <= 31 {
                    bib.day as u8
                } else {
                    1
                };
                BOOT_INFO.boot_hour = if bib.hour >= 0 && bib.hour < 24 {
                    bib.hour as u8
                } else {
                    0
                };
                BOOT_INFO.boot_minute = if bib.minute >= 0 && bib.minute < 60 {
                    bib.minute as u8
                } else {
                    0
                };
                BOOT_INFO.boot_second = if bib.second >= 0 && bib.second < 60 {
                    bib.second as u8
                } else {
                    0
                };
                BOOT_INFO.boot_timezone = 0; // BIOS RTC is local/unknown timezone

                // Execute the Kernel, passing a pointer to the BootInfo struct.
                // This function call will never return...
                execute_kernel(&raw const BOOT_INFO);
            }
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
