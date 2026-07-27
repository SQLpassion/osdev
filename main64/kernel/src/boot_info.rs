//! Unified Boot Information structure shared between bootloaders and kernel.
//!
//! This module defines the stable `#[repr(C)]` binary interface (ABI) used to pass
//! system diagnostics, memory maps, and graphics configuration from the boot stage
//! (such as BIOS `kaosldr_64` or UEFI `kaosldr_uefi`) into the 64-bit kernel.

/// Type of active video display mode selected by the bootloader.
///
/// Configured as `#[repr(u32)]` to establish a stable 4-byte size and layout
/// across separate build compilation boundaries.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoModeType {
    /// Legacy VGA text mode (typically 80x25 character grid at physical address 0xB8000).
    VgaText = 0,

    /// Modern linear graphics pixel framebuffer (supports BIOS VBE and UEFI GOP).
    Framebuffer = 1,
}

/// Color channel layout of the pixels in the linear graphics framebuffer.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Red in byte 0, Green in byte 1, Blue in byte 2, Reserved in byte 3 (little-endian: 0x00BBGGRR).
    Rgb = 0,

    /// Blue in byte 0, Green in byte 1, Red in byte 2, Reserved in byte 3 (little-endian: 0x00RRGGBB).
    Bgr = 1,

    /// Custom bitmask format.
    Bitmask = 2,

    /// Framebuffer lacks direct memory access; Blt operations only.
    BltOnly = 3,
}

/// Detailed configuration parameters of the active linear graphics framebuffer.
///
/// Guaranteed to match standard C ABI layout. Only populated and valid when
/// the active `video_type` in [`BootInfo`] is set to [`VideoModeType::Framebuffer`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    /// Physical memory base address of the linear framebuffer.
    pub base_address: u64,

    /// Total capacity of the framebuffer memory window in bytes.
    pub size: usize,

    /// Active horizontal resolution in pixels.
    pub width: u32,

    /// Active vertical resolution in pixels.
    pub height: u32,

    /// Total number of pixels per horizontal scanline (includes stride/padding alignment).
    pub pixels_per_scanline: u32,

    /// Color channel pixel format representation.
    pub pixel_format: PixelFormat,
}

/// A single standardized memory map entry representing a physical address range.
///
/// Abstracts away BIOS E820 and UEFI memory descriptor layouts into a unified,
/// simplified representation of physical memory regions.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnifiedMemoryEntry {
    /// Physical starting address of this memory region.
    pub start: u64,

    /// Size of the physical memory region in bytes.
    pub size: u64,

    /// If true, this region is general-purpose usable RAM. If false, it is reserved
    /// by firmware, ACPI, memory-mapped I/O, or contains bad blocks.
    pub is_usable: bool,
}

/// The root Boot Information structure passed from the bootloader.
///
/// Contains critical parameters required to bring up the core kernel subsystems,
/// including memory maps for the Physical Memory Manager (PMM) and dimensions for
/// the graphics framebuffer.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    /// Magic signature to validate the structure layout and source (must match `0x4B414F535F424F4F` / "KAOS_BOO").
    pub magic: u64,

    /// Selected video mode type (e.g. `VgaText` or `Framebuffer`).
    pub video_type: VideoModeType,

    /// Framebuffer details (only valid and populated when `video_type` is `Framebuffer`).
    pub fb_info: FramebufferInfo,

    /// 64-bit physical address pointing to the start of the `UnifiedMemoryEntry` array.
    pub memory_map_addr: u64,

    /// The number of entries populated in the memory map array.
    pub memory_map_len: u32,

    /// Total size of the loaded kernel binary in bytes.
    pub kernel_size: u64,

    /// Physical base address of the dedicated pre-allocated PMM metadata region.
    ///
    /// If non-zero, contains the location of a region reserved by the bootloader
    /// to store PMM bitmaps/metadata, preventing overflow issues in early memory management.
    /// If zero, the kernel falls back to allocating metadata dynamically.
    pub pmm_metadata_base: u64,

    /// Size in bytes of the reserved PMM metadata region (0 if not provided).
    pub pmm_metadata_size: u64,

    /// Year when the system was booted.
    pub boot_year: u16,

    /// Month when the system was booted.
    pub boot_month: u8,

    /// Day when the system was booted.
    pub boot_day: u8,

    /// Hour when the system was booted.
    pub boot_hour: u8,

    /// Minute when the system was booted.
    pub boot_minute: u8,

    /// Second when the system was booted.
    pub boot_second: u8,

    /// Timezone offset at boot time (minutes relative to UTC).
    pub boot_timezone: i16,
}

/// Global atomic pointer to the active BootInfo structure.
///
/// Initialized during early boot in `KernelMain` once a valid `BootInfo` block has
/// been validated by checking its magic signature. Subsystems (such as the Physical
/// Memory Manager) read this pointer to access firmware tables.
pub static BOOT_INFO_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Tracks whether the linear framebuffer reported in `BootInfo` has actually been
/// mapped into the kernel address space yet.
///
/// `BOOT_INFO_PTR` is published (and `BootInfo.video_type` already reads
/// `Framebuffer` on a BIOS+VBE boot) long before `map_framebuffer()` in `main.rs`
/// runs — `gdt::init()`, `fpu::init()`, `pmm::init()`, `interrupts::init()`,
/// `vmm::init()`, and `heap::init()` all execute in between. A panic during any of
/// those early steps must NOT let the panic handler treat the framebuffer's base
/// address as a valid, writable pointer: on BIOS+VBE that address lives outside the
/// bootstrap loader's low identity map, and no page-fault handler is installed that
/// early, so a write would triple-fault the CPU with zero diagnostic output.
///
/// Set to `true` (with `Release` ordering) only at the very end of `map_framebuffer()`,
/// after every page of the framebuffer has been mapped. Readers (e.g. the panic
/// handler) may use `Relaxed` loads: the flag itself is the synchronization point —
/// it only ever transitions `false -> true`, and a stale `false` read merely causes
/// the safe VGA-text fallback to be used instead of the framebuffer.
pub static FRAMEBUFFER_MAPPED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

impl BootInfo {
    /// Returns a static reference to the active `BootInfo` structure, if it has been validated and published.
    pub fn get() -> Option<&'static BootInfo> {
        let ptr = BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Acquire);
        if ptr == 0 {
            None
        } else {
            // SAFETY:
            // - If the pointer is non-zero, it was validated in `KernelMain` via magic check.
            // - The boot loader guarantees the memory is valid and immutable for the kernel's lifetime.
            Some(unsafe { &*(ptr as *const BootInfo) })
        }
    }
}

/// Decides whether it is currently safe to render panic diagnostics onto the linear
/// framebuffer, as opposed to falling back to the legacy VGA text-mode writer.
///
/// This is the single seam shared by the panic path's writer-selection logic and this
/// module's tests: it combines the two independent facts that must both hold before any
/// code touches `fb_info.base_address` as a live pointer:
/// 1. The active boot actually selected a linear framebuffer (`video_type ==
///    Framebuffer` with a non-zero base address) rather than legacy VGA text mode.
/// 2. `map_framebuffer()` has already run to completion and published
///    [`FRAMEBUFFER_MAPPED`] — otherwise the address is a physical address that may sit
///    outside the bootstrap loader's low identity map (BIOS+VBE case), with no
///    page-fault handler installed early enough to survive a wild write.
///
/// Checking (2) first is deliberate: on a BIOS+VBE boot, `BootInfo.video_type` already
/// reads `Framebuffer` the instant `BOOT_INFO_PTR` is published in `KernelMain` — long
/// before `gdt::init()`, `pmm::init()`, `interrupts::init()`, `vmm::init()`, or
/// `map_framebuffer()` run. A panic during any of those early steps must not be able to
/// pick the framebuffer writer just because `video_type` looks right.
pub fn framebuffer_panic_writer_available() -> bool {
    // Step 1: gate on the mapping flag first. `Relaxed` is sufficient here: the flag
    // only ever transitions `false -> true`, and the `Release` store in
    // `map_framebuffer()` is the sole writer, so a stale `false` read merely defers
    // to the always-safe VGA-text fallback rather than causing a hazard.
    if !FRAMEBUFFER_MAPPED.load(core::sync::atomic::Ordering::Relaxed) {
        return false;
    }

    // Step 2: only once mapping is confirmed, check whether the boot info actually
    // describes a usable linear framebuffer.
    match BootInfo::get() {
        Some(bi) => bi.video_type == VideoModeType::Framebuffer && bi.fb_info.base_address != 0,
        None => false,
    }
}
