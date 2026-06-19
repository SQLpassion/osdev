//! Unified Boot Information structure shared between bootloaders and kernel.

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoModeType {
    VgaText = 0,
    GopFramebuffer = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub base_address: u64,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scanline: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnifiedMemoryEntry {
    pub start: u64,
    pub size: u64,
    pub is_usable: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    /// Magic signature to validate the structure (e.g. 0x4B414F535F424F4F)
    pub magic: u64,

    /// Selected video mode type (VgaText or GopFramebuffer)
    pub video_type: VideoModeType,

    /// Framebuffer details, only valid when video_type is GopFramebuffer
    pub fb_info: FramebufferInfo,

    /// Physical address of the memory map array
    pub memory_map_addr: u64,

    /// Number of memory map entries
    pub memory_map_len: u32,

    /// Total kernel size loaded into memory
    pub kernel_size: u64,
}
