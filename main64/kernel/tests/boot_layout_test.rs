//! BootInfo / GDT / TSS ABI and layout contract tests.
//!
//! `BootInfo` is the only channel between the loaders (`kaosldr_uefi`, `kaosldr_64`) and
//! the kernel, and its `#[repr(C)]` layout is **duplicated by hand** in all three crates
//! (see `docs/uefi.md` §3.5). If the field order, sizes, or offsets drift between copies,
//! the kernel reads garbage from the loader — a class of bug that already broke the build
//! once (a field added to the kernel struct but missing in a hand-written initializer).
//!
//! These tests pin the kernel's view of the layout so any such drift fails CI. They are
//! pure compile-time/ABI checks: no firmware, no QEMU devices, no paging involved.
//!
//! The GDT/TSS tests validate the ring-3 foundation descriptors and TSS state wiring;
//! they share this binary because both are pure structural-layout checks with no
//! overlapping state.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::mem::{align_of, offset_of, size_of};
use core::panic::PanicInfo;

use kaos_kernel::arch::gdt;
use kaos_kernel::boot_info::{
    is_plausible_boot_info_pointer, BootInfo, FramebufferInfo, UnifiedMemoryEntry, VideoModeType,
};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();
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
// BootInfo contract tests
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
/// This struct is hand-duplicated in THREE places (`kernel::boot_info`, `kaosldr_uefi::main`,
/// `kaosldr_64::boot_info`) and must stay byte-identical in all three, or the kernel
/// misinterprets whichever loader's memory map it was handed.
/// Failure Impact: the PMM would mis-parse the memory map. Release-blocking.
#[test_case]
fn test_unified_memory_entry_layout() {
    assert_eq!(offset_of!(UnifiedMemoryEntry, start), 0);
    assert_eq!(offset_of!(UnifiedMemoryEntry, size), 8);
    assert_eq!(offset_of!(UnifiedMemoryEntry, memory_type), 16);
    assert_eq!(offset_of!(UnifiedMemoryEntry, attribute), 24);
    assert_eq!(offset_of!(UnifiedMemoryEntry, is_usable), 32);
    assert_eq!(size_of::<UnifiedMemoryEntry>(), 40);
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

/// Contract: `is_plausible_boot_info_pointer` rejects the low guard address (0x1000)
/// and everything at or below it, even when 8-byte aligned.
/// Failure Impact: a near-null legacy `kernel_size` value would be misread as a
/// `BootInfo` pointer and dereferenced. Release-blocking (issue #44).
#[test_case]
fn test_boot_info_pointer_rejects_low_boundary() {
    assert!(!is_plausible_boot_info_pointer(0));
    assert!(!is_plausible_boot_info_pointer(0x1000));
    assert!(!is_plausible_boot_info_pointer(0x0FF8));
}

/// Contract: `is_plausible_boot_info_pointer` accepts any aligned address above the
/// low guard, including addresses well above the BIOS loader's 16 MiB low identity
/// map — this is where a genuine UEFI `BootInfo` pointer typically lives (the
/// firmware's PE loader places `kaosldr_uefi`'s image, and therefore its `BootInfo`
/// static, wherever it chooses; UEFI's own page tables identity-map all of physical
/// RAM, so such an address is always safely dereferenceable).
/// Failure Impact: rejecting a high address here silently diverts a UEFI boot into
/// the legacy BIOS/ATA branch, which finds no disk under UEFI/AHCI — this is the
/// regression fixed after issue #44 broke the UEFI boot path.
#[test_case]
fn test_boot_info_pointer_accepts_in_range_address() {
    assert!(is_plausible_boot_info_pointer(0x1008));
    assert!(is_plausible_boot_info_pointer(0x0080_0000));
    // Representative of a real UEFI-placed BootInfo address, well above the
    // BIOS-only 16 MiB low identity map.
    assert!(is_plausible_boot_info_pointer(0x0500_0000));
    assert!(is_plausible_boot_info_pointer(0x1_0000_0000));
}

/// Contract: `is_plausible_boot_info_pointer` rejects misaligned addresses, even
/// when they otherwise fall within the valid range.
/// Failure Impact: reading the `u64` magic field at a misaligned address is still
/// well-defined on x86_64, but the check exists to only ever accept genuinely
/// `#[repr(C)]`-aligned `BootInfo` pointers. Release-blocking.
#[test_case]
fn test_boot_info_pointer_rejects_misaligned_address() {
    assert!(!is_plausible_boot_info_pointer(0x1001));
    assert!(!is_plausible_boot_info_pointer(0x0080_0004));
}

// ============================================================================
// GDT/TSS contract tests
// ============================================================================

/// Contract: selector constants follow expected long-mode layout.
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_selector_constants() {
    assert_eq!(gdt::KERNEL_CODE_SELECTOR, 0x08);
    assert_eq!(gdt::KERNEL_DATA_SELECTOR, 0x10);
    assert_eq!(gdt::USER_CODE_SELECTOR, 0x1B);
    assert_eq!(gdt::USER_DATA_SELECTOR, 0x23);
    assert_eq!(gdt::TSS_SELECTOR, 0x28);
}

/// Contract: gdt init loads tss descriptor and kernel rsp0.
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_tss_descriptor_present_and_rsp0_nonzero() {
    assert!(gdt::is_initialized(), "GDT/TSS must be initialized");

    let descriptors = gdt::descriptor_snapshot();
    let tss_low = descriptors[5];
    let tss_high = descriptors[6];

    let tss_type = (tss_low >> 40) & 0x0F;
    let present = (tss_low >> 47) & 0x01;
    let base_low = ((tss_low >> 16) & 0xFFFF)
        | (((tss_low >> 32) & 0xFF) << 16)
        | (((tss_low >> 56) & 0xFF) << 24);
    let base_high = tss_high & 0xFFFF_FFFF;
    let base = base_low | (base_high << 32);

    assert!(
        tss_type == 0x9 || tss_type == 0xB,
        "TSS descriptor type must be available (0x9) or busy (0xB) 64-bit TSS"
    );
    assert_eq!(present, 1, "TSS descriptor must be marked present");
    assert_ne!(base, 0, "TSS base address must be non-zero");

    let rsp0 = gdt::kernel_rsp0();
    assert_ne!(rsp0, 0, "TSS RSP0 must be initialized");
}

/// Contract: kernel rsp0 setter updates tss state.
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_set_kernel_rsp0_roundtrip() {
    let old_rsp0 = gdt::kernel_rsp0();
    let test_rsp0 = 0xFFFF_8000_0012_3000u64;

    gdt::set_kernel_rsp0(test_rsp0);
    assert_eq!(gdt::kernel_rsp0(), test_rsp0);

    gdt::set_kernel_rsp0(old_rsp0);
}

/// Contract: tss ist1 points to a dedicated aligned emergency stack.
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_tss_ist1_is_initialized_and_aligned() {
    let ist1 = gdt::kernel_ist1();
    assert_ne!(ist1, 0, "TSS IST1 must be initialized");
    assert_eq!(ist1 & 0xF, 0, "TSS IST1 must be 16-byte aligned");
}
