//! KAOS Rust Kernel - Main Entry Point
//!
//! This is the kernel entry point called by the bootloader.
//! The bootloader sets up long mode (64-bit) and jumps here.

#![no_std]
#![no_main]

mod apps;
mod arch;
mod drivers;
mod panic;

use crate::arch::interrupts;
use crate::arch::power;
use core::ptr;
use core::fmt::Write;
use drivers::keyboard;
use drivers::screen::{Color, Screen};

const PAGE_SIZE: u64 = 4096;

const BIB_OFFSET: usize = 0x1000;
const MEMORYMAP_OFFSET: usize = 0x1200;
const KERNEL_OFFSET: u64 = 0x100000;
const MARK_1MB: u64 = 0x100000;
const KERNEL_VIRT_BASE: u64 = 0xFFFF800000000000;

extern "C" {
    static __bss_end: u8;
}

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
    physical_memory_layout: *mut PmmLayoutHeader
}

#[repr(C)]
struct BiosMemoryRegion {
    start: u64,
    size: u64,
    region_type: u32,
}

/// Kernel entry point - called from bootloader (kaosldr_64)
///
/// The function signature matches the C version:
/// `void KernelMain(int KernelSize)`
///
/// # Safety
/// This function is called from assembly with the kernel size in RDI.
#[no_mangle]
#[link_section = ".text.boot"]
#[allow(unconditional_panic)]
pub extern "C" fn KernelMain(kernel_size: u64) -> ! {
    unsafe {
         init_physical_memory_manager(kernel_size);
    }

    // Initialize interrupt handling and the keyboard ring buffer.
    interrupts::init();
    interrupts::register_irq_handler(interrupts::IRQ1_VECTOR, |_| {
        keyboard::handle_irq();
    });
    keyboard::init();
    interrupts::enable();

    // Initialize the screen
    let mut screen = Screen::new();
    screen.clear();

    // Print welcome message
    screen.set_color(Color::LightGreen);
    writeln!(screen, "========================================").unwrap();
    writeln!(screen, "    KAOS - Klaus' Operating System").unwrap();
    writeln!(screen, "         Rust Kernel v0.1.0").unwrap();
    writeln!(screen, "========================================").unwrap();
    screen.set_color(Color::White);
    writeln!(screen, "Kernel loaded successfully!").unwrap();
    writeln!(screen, "Kernel size: {} bytes\n", kernel_size).unwrap();

    // Execute the command prompt loop
    command_prompt_loop(&mut screen);
}

/// Infinite prompt loop using read_line; echoes entered lines.
fn command_prompt_loop(screen: &mut Screen) -> ! {
    loop {
        write!(screen, "> ").unwrap();

        let mut buf = [0u8; 128];
        let len = keyboard::read_line(screen, &mut buf);

        if let Ok(line) = core::str::from_utf8(&buf[..len]) {
            execute_command(screen, line);
        } else {
            writeln!(screen, "(invalid UTF-8)").unwrap();
        }
    }
}

/// Execute a simple command from a line of input.
fn execute_command(screen: &mut Screen, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        screen.print_char(b'\n');
        return;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap();

    match cmd {
        "help" => {
            writeln!(screen, "Commands:\n").unwrap();
            writeln!(screen, "  help            - show this help").unwrap();
            writeln!(screen, "  echo <text>     - print text").unwrap();
            writeln!(screen, "  cls             - clear screen").unwrap();
            writeln!(screen, "  color <name>    - set color (white, cyan, green)").unwrap();
            writeln!(screen, "  apps            - list available applications").unwrap();
            writeln!(screen, "  run <app>       - run an application").unwrap();
            writeln!(screen, "  meminfo         - display BIOS memory map").unwrap();
            writeln!(screen, "  shutdown        - shutdown the system").unwrap();
        }
        "echo" => {
            let rest = line[cmd.len()..].trim_start();
            if !rest.is_empty() {
                writeln!(screen, "{}", rest).unwrap();
            } else {
                screen.print_char(b'\n');
            }
        }
        "cls" | "clear" => {
            screen.clear();
        }
        "color" => {
            if let Some(name) = parts.next() {
                if name.eq_ignore_ascii_case("white") {
                    screen.set_color(Color::White);
                } else if name.eq_ignore_ascii_case("cyan") {
                    screen.set_color(Color::LightCyan);
                } else if name.eq_ignore_ascii_case("green") {
                    screen.set_color(Color::LightGreen);
                } else {
                    writeln!(screen, "Unknown color: {}", name).unwrap();
                }
            } else {
                writeln!(screen, "Usage: color <white|cyan|green>").unwrap();
            }
        }
        "shutdown" => {
            writeln!(screen, "Shutting down...").unwrap();
            power::shutdown();
        }
        "apps" => {
            apps::list_apps(screen);
        }
        "run" => {
            if let Some(app_name) = parts.next() {
                if !apps::run_app(app_name, screen) {
                    writeln!(screen, "Unknown app: {}", app_name).unwrap();
                    writeln!(screen, "Use 'apps' to list available applications.").unwrap();
                }
            } else {
                writeln!(screen, "Usage: run <appname>").unwrap();
                writeln!(screen, "Use 'apps' to list available applications.").unwrap();
            }
        }
        "meminfo" => {
            print_memory_map(screen);
        }
        _ => {
            writeln!(screen, "Unknown command: {}", cmd).unwrap();
        }
    }
}

/// Prints the memory map that we have obtained from the BIOS in x16 Real Mode.
fn print_memory_map(screen: &mut Screen) {
    // Mutable view so we can mirror the C code: compute MaxMemory and AvailablePageFrames here.
    let bib = unsafe {
         &mut *(BIB_OFFSET as *mut BiosInformationBlock)
    };
    
    let region = MEMORYMAP_OFFSET as *const BiosMemoryRegion;
    
    let entry_count = bib.memory_map_entries as usize;

    // Reset and recompute MaxMemory / AvailablePageFrames like the C physical memory manager.
    bib.max_memory = 0;
    bib.available_page_frames = 0;

    // Print header
    writeln!(screen, "{} Memory Map entries found.", entry_count).unwrap();
    
    // Loop over each entry
    for i in 0..entry_count {
        let current_region = unsafe {
            &*region.add(i)
        };
        
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

    unsafe {
        let frames = [
            allocate_page_frame(),
            allocate_page_frame(),
            allocate_page_frame(),
        ];

        for (i, pf) in frames.iter().enumerate() {
            match pf {
                Some(f) => writeln!(
                    screen,
                    "[{}] pfn={} phys=0x{:x} region={}",
                    i, f.pfn, f.physical_address(), f.region_index
                ).unwrap(),
                None => writeln!(screen, "[{}] allocation failed", i).unwrap(),
            }
        } 
    } 
}




/// Represents an allocated page frame with its PFN and region info.
/// This handle is returned by `allocate_page_frame` and passed to `release_page_frame`.
#[derive(Clone, Copy, Debug)]
pub struct PageFrame {
    /// Page Frame Number (physical address / PAGE_SIZE)
    pub pfn: u64,
    /// Internal: index of the memory region this frame belongs to
    region_index: u32,
}

impl PageFrame {
    /// Returns the physical address of this page frame.
    #[inline]
    pub fn physical_address(&self) -> u64 {
        self.pfn * PAGE_SIZE
    }
}

#[repr(C)]
struct PmmLayoutHeader {
    /// Number of usable memory regions described by the PMM.
    region_count: u32,
    /// Keeps the following region array 8-byte aligned.
    padding: u32,
}

#[repr(C)]
struct PmmRegion {
    /// Physical start address of the region.
    start: u64,
    /// Total number of page frames in this region.
    frames_total: u64,
    /// Current number of free page frames in this region.
    frames_free: u64,
    /// Physical address of the bitmap for this region.
    bitmap_start: u64,
    /// Size of the bitmap in bytes (aligned to 8).
    bitmap_bytes: u64,
}

#[inline]
fn align_up(x: u64, align: u64) -> u64 {
    (x + align - 1) & !(align - 1)
}

#[inline]
fn virt_to_phys(addr: u64) -> u64 {
    if addr >= KERNEL_VIRT_BASE {
        addr - KERNEL_VIRT_BASE
    } else {
        addr
    }
}

#[inline]
unsafe fn set_bit(idx: u64, base: *mut u64) {
    let word = base.add((idx / 64) as usize);
    let mask = 1u64 << (idx % 64);
    *word |= mask;
}

#[inline]
unsafe fn clear_bit(idx: u64, base: *mut u64) {
    let word = base.add((idx / 64) as usize);
    let mask = 1u64 << (idx % 64);
    *word &= !mask;
}

unsafe fn pmm_regions<'a>(header: *mut PmmLayoutHeader) -> &'a mut [PmmRegion] {
    let count = (*header).region_count as usize;
    let regions_ptr = (header as *mut u8).add(core::mem::size_of::<PmmLayoutHeader>())
        as *mut PmmRegion;
    core::slice::from_raw_parts_mut(regions_ptr, count)
}

pub unsafe fn init_physical_memory_manager(kernel_size: u64) {
    let _ = kernel_size;
    let bib = (BIB_OFFSET as *mut BiosInformationBlock).as_mut().unwrap();
    let region = MEMORYMAP_OFFSET as *const BiosMemoryRegion;

    // Place PMM layout right after the kernel image (including BSS), aligned to 4K.
    let kernel_end_virt = &__bss_end as *const u8 as u64;
    let kernel_end_phys = virt_to_phys(kernel_end_virt);
    let start_addr = align_up(kernel_end_phys, PAGE_SIZE);
    let header = start_addr as *mut PmmLayoutHeader;
    (*header).region_count = 0;
    (*header).padding = 0;
    bib.physical_memory_layout = header;

    // Count usable regions first.
    let mut count = 0u32;
    for i in 0..bib.memory_map_entries as usize {
        let r = &*region.add(i);
        if r.region_type == 1 && r.start >= MARK_1MB {
            count += 1;
        }
    }
    (*header).region_count = count;

    let regions = pmm_regions(header);

    // Fill regions.
    let mut idx = 0usize;
    for i in 0..bib.memory_map_entries as usize {
        let r = &*region.add(i);
        if r.region_type == 1 && r.start >= MARK_1MB {
            let frames = r.size / PAGE_SIZE;
            let bitmap_bytes = align_up((frames + 7) / 8, 8);
            regions[idx] = PmmRegion {
                start: r.start,
                frames_total: frames,
                frames_free: frames,
                bitmap_start: 0,
                bitmap_bytes,
            };
            idx += 1;
        }
    }

    // Bitmaps right after the region array.
    let mut bitmap_base = (regions.as_ptr() as u64)
        + (count as u64) * (core::mem::size_of::<PmmRegion>() as u64);

    for r in regions.iter_mut() {
        r.bitmap_start = bitmap_base;
        core::ptr::write_bytes(r.bitmap_start as *mut u8, 0, r.bitmap_bytes as usize);
        bitmap_base += r.bitmap_bytes;
    }

    // Mark kernel + PMM metadata used.
    let used_frames = get_used_page_frames(header);
    for _ in 0..used_frames {
        let _ = allocate_page_frame();
    }
}

/// Allocates a single page frame from the first available region.
/// Returns `Some(PageFrame)` on success, or `None` if no free frames exist.
pub unsafe fn allocate_page_frame() -> Option<PageFrame> {
    let bib = (BIB_OFFSET as *mut BiosInformationBlock).as_mut().unwrap();
    let header = bib.physical_memory_layout;
    let regions = pmm_regions(header);

    for (idx, r) in regions.iter_mut().enumerate() {
        if r.frames_free == 0 {
            continue;
        }
        let words = (r.bitmap_bytes / 8) as usize;
        let bitmap = r.bitmap_start as *mut u64;
        for w in 0..words {
            let val = *bitmap.add(w);
            if val != u64::MAX {
                let free_bit = (!val).trailing_zeros() as u64;
                let bit_idx = (w as u64) * 64 + free_bit;
                if bit_idx < r.frames_total {
                    set_bit(bit_idx, bitmap);
                    r.frames_free -= 1;
                    return Some(PageFrame {
                        pfn: r.start / PAGE_SIZE + bit_idx,
                        region_index: idx as u32,
                    });
                }
            }
        }
    }

    None
}

/// Releases a previously allocated page frame back to the pool.
/// The `PageFrame` handle contains all information needed to free the frame.
pub unsafe fn release_page_frame(frame: PageFrame) {
    let bib = (BIB_OFFSET as *mut BiosInformationBlock).as_mut().unwrap();
    let header = bib.physical_memory_layout;
    let regions = pmm_regions(header);
    let r = &mut regions[frame.region_index as usize];
    let bitmap = r.bitmap_start as *mut u64;
    let bit_idx = frame.pfn - (r.start / PAGE_SIZE);
    clear_bit(bit_idx, bitmap);
    r.frames_free += 1;
}

// Calculate pages used by kernel + PMM metadata
pub unsafe fn get_used_page_frames(header: *mut PmmLayoutHeader) -> u64 {
    let regions = pmm_regions(header);
    if regions.is_empty() {
        return 0;
    }

    let last = &regions[regions.len() - 1];
    let last_used = last.bitmap_start + last.bitmap_bytes;

    if last_used <= KERNEL_OFFSET {
        return 0;
    }

    (last_used - KERNEL_OFFSET) / PAGE_SIZE + 1
}



/*
                    PHYSICAL MEMORY LAYOUT (RUST PMM)
    ═══════════════════════════════════════════════════════════════════

    0x00000000 ┌─────────────────────────────────────────────────────┐
               │                                                     │
               │              Real Mode Memory                       │
               │         (IVT, BDA, BIOS Data, etc.)                 │
               │                                                     │
    0x00001000 ├─────────────────────────────────────────────────────┤
               │  BiosInformationBlock (BIB_OFFSET)                  │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ year: i32                                   │    │
               │  │ month: i16                                  │    │
               │  │ day: i16                                    │    │
               │  │ hour: i16                                   │    │
               │  │ minute: i16                                 │    │
               │  │ second: i16                                 │    │
               │  │ memory_map_entries: i16                     │    │
               │  │ max_memory: i64                             │    │
               │  │ available_page_frames: i64                  │    │
               │  │ physical_memory_layout: *mut ──────────────────────┐
               │  └─────────────────────────────────────────────┘    │ │
    0x00001200 ├─────────────────────────────────────────────────────┤ │
               │  BIOS Memory Map (MEMORYMAP_OFFSET)                 │ │
               │  ┌─────────────────────────────────────────────┐    │ │
               │  │ BiosMemoryRegion[0]                         │    │ │
               │  │   start: u64                                │    │ │
               │  │   size: u64                                 │    │ │
               │  │   region_type: u32                          │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ BiosMemoryRegion[1]                         │    │ │
               │  │   ...                                       │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ BiosMemoryRegion[N]                         │    │ │
               │  │   ...                                       │    │ │
               │  └─────────────────────────────────────────────┘    │ │
               │                                                     │ │
               │         ... (reserved/other BIOS areas) ...         │ │
               │                                                     │ │
    0x00100000 ├═════════════════════════════════════════════════════┤ │
  (1 MB)       │                                                     │ │
 KERNEL_OFFSET │                    KERNEL CODE                      │ │
               │                                                     │ │
               │              (.text, .rodata, .data, .bss)          │ │
               │                                                     │ │
               │                   kernel_size bytes                 │ │
               │                                                     │ │
    0x00100000 ├─────────────────────────────────────────────────────┤ │
        +      │                                                     │ │
  kernel_size  │            (padding to 4KB alignment)               │ │
   (aligned)   │                                                     │ │
               ├═════════════════════════════════════════════════════┤◄┘
               │                                                     │
               │          PmmLayoutHeader + PmmRegion[]              │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ region_count: u32 (4 bytes)                 │    │
               │  │ padding: u32 (4 bytes)                      │    │
               │  ├─────────────────────────────────────────────┤    │
               │  │ regions[0]: PmmRegion                       │    │
               │  │   ┌───────────────────────────────────────┐ │    │
               │  │   │ start: u64                            │ │    │
               │  │   │ frames_total: u64                     │ │    │
               │  │   │ frames_free: u64                      │ │    │
               │  │   │ bitmap_start: u64 ─────────────────────────────┐
               │  │   │ bitmap_bytes: u64                     │ │    │ │
               │  │   └───────────────────────────────────────┘ │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ regions[1]: PmmRegion                       │    │ │
               │  │   (same fields as above) ─────────────────────────────┐
               │  ├─────────────────────────────────────────────┤    │ │  │
               │  │                ...                          │    │ │  │
               │  └─────────────────────────────────────────────┘    │ │  │
               ├═══════════════════════════════════════════════════┤◄──┘  │
               │                                                     │    │
               │         BITMAP FOR REGION 0                         │    │
               │  ┌─────────────────────────────────────────────┐    │    │
               │  │ Each bit = 1 page frame (4KB)               │    │    │
               │  │                                             │    │    │
               │  │ Word 0: [bits 0-63]                         │    │    │
               │  │   bit 0: 1=used, 0=free (PFN 0 of region)   │    │    │
               │  │   bit 1: 1=used, 0=free (PFN 1 of region)   │    │    │
               │  │   ...                                       │    │    │
               │  │ Word 1: [bits 64-127]                       │    │    │
               │  │   ...                                       │    │    │
               │  │ Word N: [bits N*64 - (N+1)*64-1]            │    │    │
               │  │                                             │    │    │
               │  │ Size = bitmap_bytes                         │    │    │
               │  └─────────────────────────────────────────────┘    │    │
               ├─────────────────────────────────────────────────────┤◄───┘
               │                                                     │
               │         BITMAP FOR REGION 1                         │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ (same structure as above)                   │    │
               │  │ Size = region[1].bitmap_bytes               │    │
               │  └─────────────────────────────────────────────┘    │
               ├─────────────────────────────────────────────────────┤
               │                                                     │
               │         BITMAP FOR REGION N...                      │
               │                                                     │
               ├═════════════════════════════════════════════════════┤
               │                                                     │
               │                                                     │
               │            FREE PHYSICAL MEMORY                     │
               │                                                     │
               │      (allocatable via allocate_page_frame())        │
               │                                                     │
               │                                                     │
               └─────────────────────────────────────────────────────┘
               


    ═══════════════════════════════════════════════════════════════════
                   PageFrame HANDLE (returned by allocate)
    ═══════════════════════════════════════════════════════════════════

         ┌────────────────────────────────────────────────────────┐
         │  PageFrame (12 bytes logical, may be padded)           │
         │  ┌──────────────────────────────────────────────────┐  │
         │  │ pfn: u64 (8 bytes)                               │  │
         │  │   Page Frame Number = phys_addr / 4096           │  │
         │  ├──────────────────────────────────────────────────┤  │
         │  │ region_index: u32 (4 bytes, private)             │  │
         │  │   Index into memory_regions[] array              │  │
         │  └──────────────────────────────────────────────────┘  │
         │                                                        │
         │  Methods:                                              │
         │    physical_address() -> pfn * 4096                    │
         └────────────────────────────────────────────────────────┘


    ═══════════════════════════════════════════════════════════════════
                              BITMAP ENCODING
    ═══════════════════════════════════════════════════════════════════

    Each u64 word in bitmap:
    ┌────┬────┬────┬────┬────┬────┬────┬────┬─────────┬────┬────┬────┐
    │ 63 │ 62 │ 61 │ 60 │ 59 │ 58 │ 57 │ 56 │   ...   │  2 │  1 │  0 │
    └────┴────┴────┴────┴────┴────┴────┴────┴─────────┴────┴────┴────┘
      │                                                            │
      │    1 = Page frame ALLOCATED (in use)                       │
      │    0 = Page frame FREE (available)                         │
      │                                                            │
      └── bit 63 = PFN (word_index * 64 + 63)                      │
                                           bit 0 = PFN (word_index * 64 + 0)

    Example: For region starting at 0x100000 (1MB):
      Word 0, Bit 0  →  PFN = 0x100000/4096 + 0   = 256   → Phys: 0x100000
      Word 0, Bit 1  →  PFN = 0x100000/4096 + 1   = 257   → Phys: 0x101000
      Word 0, Bit 63 →  PFN = 0x100000/4096 + 63  = 319   → Phys: 0x13F000
      Word 1, Bit 0  →  PFN = 0x100000/4096 + 64  = 320   → Phys: 0x140000


    ═══════════════════════════════════════════════════════════════════
                         CONCRETE MEMORY EXAMPLE
    ═══════════════════════════════════════════════════════════════════

    Assuming: kernel_size = 0x8000 (32KB), 1 memory region with 128MB

    0x00100000  ┌──────────────────────────────────┐ ◄─ KERNEL_OFFSET
                │         Kernel (32 KB)           │
    0x00108000  ├──────────────────────────────────┤ ◄─ kernel end
                │      (padding to 4KB align)      │
    0x00109000  ├──────────────────────────────────┤ ◄─ PmmLayoutHeader
                │  region_count: 1                 │    (4 bytes)
                │  padding: 0                      │    (4 bytes)
    0x00109008  │  regions[0]:                     │    (40 bytes)
                │    start: 0x100000               │
                │    frames_total: 32768           │    (128MB / 4KB)
                │    frames_free: 32768            │
                │    bitmap_start: 0x00109030 ──────────┐
                │    bitmap_bytes: 4096            │    │
    0x00109030  ├──────────────────────────────────┤◄───┘
                │                                  │
                │     Bitmap (4096 bytes)          │
                │     32768 bits = 32768 pages     │
                │                                  │
    0x0010A030  ├──────────────────────────────────┤
                │                                  │
                │     FREE MEMORY STARTS HERE      │
                │                                  │
                │    (first ~10 pages marked used  │
                │     for kernel + PMM metadata)   │
                │                                  │
    0x08000000  └──────────────────────────────────┘ ◄─ End of 128MB
    (128 MB)
*/
