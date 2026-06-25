//! Unified Boot Information structure shared between bootloaders and kernel.
//!
//! This module defines the stable `#[repr(C)]` binary interface (ABI) used to pass
//! system diagnostics, memory maps, and graphics configuration from the BIOS
//! bootloader (`kaosldr_64`) into the 64-bit kernel.

#![allow(dead_code)]

/// Type of active video display mode selected by the bootloader.
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
/// Abstracts BIOS E820 memory descriptor layouts into a unified, simplified representation
/// of physical memory regions.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnifiedMemoryEntry {
    /// Physical starting address of this memory region.
    pub start: u64,

    /// Size of the physical memory region in bytes.
    pub size: u64,

    /// If true, this region is general-purpose usable RAM. If false, it is reserved.
    pub is_usable: bool,
}

/// The root Boot Information structure passed from the bootloader to the kernel.
///
/// Contains critical parameters required to bring up the core kernel subsystems,
/// including memory maps for the Physical Memory Manager (PMM) and dimensions for
/// the graphics framebuffer.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    /// Magic signature to validate the structure (must match `0x4B414F535F424F4F` / "KAOS_BOO").
    pub magic: u64,

    /// Selected video mode type (VgaText or Framebuffer).
    pub video_type: VideoModeType,

    /// Framebuffer details, only valid when video_type is Framebuffer.
    pub fb_info: FramebufferInfo,

    /// Physical address of the memory map array.
    pub memory_map_addr: u64,

    /// Number of memory map entries.
    pub memory_map_len: u32,

    /// Total kernel size loaded into memory.
    pub kernel_size: u64,

    /// Physical base of a dedicated pre-allocated PMM metadata region.
    ///
    /// If non-zero, contains the location of a region reserved by the bootloader
    /// to store PMM bitmaps/metadata, preventing overflow issues in early memory management.
    /// The BIOS loader leaves this 0 and relies on the legacy placement.
    pub pmm_metadata_base: u64,

    /// Size in bytes of the reserved PMM metadata region (0 if not provided).
    pub pmm_metadata_size: u64,
}
