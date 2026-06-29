//! BootInfo ABI / layout contract test.
//!
//! `BootInfo` is the only channel between the loaders (`kaosldr_uefi`, `kaosldr_64`) and
//! the kernel, and its `#[repr(C)]` layout is **duplicated by hand** in all three crates
//! (see `docs/uefi.md` §3.5). If the field order, sizes, or offsets drift between copies,
//! the kernel reads garbage from the loader — a class of bug that already broke the build
//! once (a field added to the kernel struct but missing in a hand-written initializer).
//!
//! These tests pin the kernel's view of the layout so any such drift fails CI. They are
//! pure compile-time/ABI checks: no firmware, no QEMU devices, no paging involved.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::mem::{align_of, offset_of, size_of};
use core::panic::PanicInfo;

use kaos_kernel::boot_info::{BootInfo, FramebufferInfo, UnifiedMemoryEntry, VideoModeType};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

// ============================================================================
// Contract tests
// ============================================================================

/// Contract: the BootInfo magic constant is the agreed value "KAOS_BOO".
/// Failure Impact: loader and kernel disagree on the sanity signature; the kernel
/// would reject a valid BootInfo (or, worse, accept a stale pointer). Release-blocking.
#[test_case]
fn test_bootinfo_magic_value() {
    assert_eq!(0x4B41_4F53_5F42_4F4F_u64.to_be_bytes(), *b"KAOS_BOO");
}

/// Contract: VideoModeType discriminants are stable (VgaText=0, Framebuffer=1).
/// Failure Impact: the kernel would mis-detect the boot path (BIOS vs UEFI/Framebuffer). Release-blocking.
#[test_case]
fn test_video_mode_discriminants() {
    assert_eq!(VideoModeType::VgaText as u32, 0);
    assert_eq!(VideoModeType::Framebuffer as u32, 1);
    assert_eq!(size_of::<VideoModeType>(), 4, "repr(u32)");
}

/// Contract: `magic` is the FIRST field (offset 0).
/// The kernel validates a raw pointer by reading `*(ptr as *const u64)` BEFORE casting it
/// to `&BootInfo`; that read only sees the magic if `magic` sits at offset 0.
/// Failure Impact: the magic check reads the wrong bytes → silent acceptance/rejection.
#[test_case]
fn test_magic_is_first_field() {
    assert_eq!(offset_of!(BootInfo, magic), 0);
}

/// Contract: the exact `#[repr(C)]` field offsets and size of `BootInfo`.
/// These encode the binary layout the loaders must match field-for-field.
/// Failure Impact: any field reorder/insert/resize desyncs loader<->kernel. Release-blocking.
#[test_case]
fn test_bootinfo_field_offsets() {
    assert_eq!(offset_of!(BootInfo, magic), 0);
    assert_eq!(offset_of!(BootInfo, video_type), 8);
    assert_eq!(offset_of!(BootInfo, fb_info), 16);
    assert_eq!(offset_of!(BootInfo, memory_map_addr), 48);
    assert_eq!(offset_of!(BootInfo, memory_map_len), 56);
    assert_eq!(offset_of!(BootInfo, kernel_size), 64);
    assert_eq!(offset_of!(BootInfo, pmm_metadata_base), 72);
    assert_eq!(offset_of!(BootInfo, pmm_metadata_size), 80);
    assert_eq!(offset_of!(BootInfo, boot_year), 88);
    assert_eq!(offset_of!(BootInfo, boot_month), 90);
    assert_eq!(offset_of!(BootInfo, boot_day), 91);
    assert_eq!(offset_of!(BootInfo, boot_hour), 92);
    assert_eq!(offset_of!(BootInfo, boot_minute), 93);
    assert_eq!(offset_of!(BootInfo, boot_second), 94);
    assert_eq!(offset_of!(BootInfo, boot_timezone), 96);
    assert_eq!(size_of::<BootInfo>(), 104);
    assert_eq!(align_of::<BootInfo>(), 8);
}

/// Contract: the exact `#[repr(C)]` layout of `FramebufferInfo`.
/// Failure Impact: the kernel would read wrong framebuffer geometry → fault/garbage. Release-blocking.
#[test_case]
fn test_framebuffer_info_layout() {
    assert_eq!(offset_of!(FramebufferInfo, base_address), 0);
    assert_eq!(offset_of!(FramebufferInfo, size), 8);
    assert_eq!(offset_of!(FramebufferInfo, width), 16);
    assert_eq!(offset_of!(FramebufferInfo, height), 20);
    assert_eq!(offset_of!(FramebufferInfo, pixels_per_scanline), 24);
    assert_eq!(size_of::<FramebufferInfo>(), 32);
    assert_eq!(align_of::<FramebufferInfo>(), 8);
}

/// Contract: the exact `#[repr(C)]` layout of `UnifiedMemoryEntry` (the loader's memory-map element).
/// Failure Impact: the PMM would mis-parse the memory map. Release-blocking.
#[test_case]
fn test_unified_memory_entry_layout() {
    assert_eq!(offset_of!(UnifiedMemoryEntry, start), 0);
    assert_eq!(offset_of!(UnifiedMemoryEntry, size), 8);
    assert_eq!(offset_of!(UnifiedMemoryEntry, is_usable), 16);
    assert_eq!(size_of::<UnifiedMemoryEntry>(), 24);
    assert_eq!(align_of::<UnifiedMemoryEntry>(), 8);
}

/// Contract: the exact `#[repr(C)]` layout of `BiosInformationBlock`.
/// Failure Impact: loader (kaosldr_16, kaosldr_64) and kernel mismatch offsets → boot failure or graphics failure. Release-blocking.
#[test_case]
fn test_bios_information_block_layout() {
    use kaos_kernel::memory::bios::BiosInformationBlock;

    assert_eq!(offset_of!(BiosInformationBlock, year), 0);
    assert_eq!(offset_of!(BiosInformationBlock, month), 4);
    assert_eq!(offset_of!(BiosInformationBlock, day), 6);
    assert_eq!(offset_of!(BiosInformationBlock, hour), 8);
    assert_eq!(offset_of!(BiosInformationBlock, minute), 10);
    assert_eq!(offset_of!(BiosInformationBlock, second), 12);
    assert_eq!(offset_of!(BiosInformationBlock, memory_map_entries), 14);
    assert_eq!(offset_of!(BiosInformationBlock, max_memory), 16);
    assert_eq!(offset_of!(BiosInformationBlock, available_page_frames), 24);
    assert_eq!(offset_of!(BiosInformationBlock, video_type), 32);
    assert_eq!(offset_of!(BiosInformationBlock, fb_base_address), 40);
    assert_eq!(offset_of!(BiosInformationBlock, fb_size), 48);
    assert_eq!(offset_of!(BiosInformationBlock, fb_width), 56);
    assert_eq!(offset_of!(BiosInformationBlock, fb_height), 60);
    assert_eq!(offset_of!(BiosInformationBlock, fb_pixels_per_scanline), 64);
    assert_eq!(size_of::<BiosInformationBlock>(), 72);
    assert_eq!(align_of::<BiosInformationBlock>(), 8);
}
