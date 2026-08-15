//! ELF64 parser contract tests (`process::elf`).
//!
//! Pure parsing/validation logic over synthetic in-memory ELF64 byte buffers
//! (built by the `builder` helpers below) — no VFS, no address-space
//! mapping. This pins the validation rules the loader requires before it is
//! allowed to trust a `PT_LOAD` segment list: correct magic/class/endianness/
//! machine/type, `p_filesz <= p_memsz`, the segment's file range and virtual
//! range both falling inside the user code window, and no two segments
//! overlapping.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::panic::PanicInfo;

use kaos_kernel::arch::{gdt, interrupts};
use kaos_kernel::memory::vmm::USER_CODE_BASE;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::elf::{parse_elf64, ElfError, PF_R, PF_W, PF_X};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();
    pmm::init(false);
    interrupts::init();
    vmm::init(false);
    heap::init(false);
    test_main();
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

const PAGE_SIZE: u64 = 4096;
const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;

/// One segment description fed to [`build_elf_image`].
struct SegSpec {
    vaddr: u64,
    flags: u32,
    /// Bytes physically present in the file for this segment.
    file_bytes: Vec<u8>,
    /// Total in-memory size (>= file_bytes.len(); the tail is BSS).
    memsz: u64,
}

/// Builds a minimal well-formed ELF64 `ET_EXEC`/`EM_X86_64` image with the
/// given segments, laid out back-to-back in the file starting right after
/// the program header table.
///
/// Returns the raw bytes; callers corrupt individual fields afterwards to
/// exercise specific rejection paths.
fn build_elf_image(entry: u64, segs: &[SegSpec]) -> Vec<u8> {
    let phoff = EHDR_SIZE as u64;
    let phnum = segs.len() as u64;
    let mut file_offset = phoff + phnum * PHDR_SIZE as u64;

    let mut offsets = Vec::new();
    for seg in segs {
        offsets.push(file_offset);
        file_offset += seg.file_bytes.len() as u64;
    }

    let mut image = vec![0u8; file_offset as usize];

    // e_ident
    image[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    image[4] = 2; // ELFCLASS64
    image[5] = 1; // ELFDATA2LSB
    image[6] = 1; // EV_CURRENT

    image[16..18].copy_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    image[18..20].copy_from_slice(&62u16.to_le_bytes()); // e_machine = EM_X86_64
    image[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    image[24..32].copy_from_slice(&entry.to_le_bytes()); // e_entry
    image[32..40].copy_from_slice(&phoff.to_le_bytes()); // e_phoff
    image[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes()); // e_phentsize
    image[56..58].copy_from_slice(&(phnum as u16).to_le_bytes()); // e_phnum

    for (i, seg) in segs.iter().enumerate() {
        let ph = phoff as usize + i * PHDR_SIZE;
        image[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
        image[ph + 4..ph + 8].copy_from_slice(&seg.flags.to_le_bytes());
        image[ph + 8..ph + 16].copy_from_slice(&offsets[i].to_le_bytes()); // p_offset
        image[ph + 16..ph + 24].copy_from_slice(&seg.vaddr.to_le_bytes()); // p_vaddr
        image[ph + 24..ph + 32].copy_from_slice(&seg.vaddr.to_le_bytes()); // p_paddr (ignored)
        image[ph + 32..ph + 40].copy_from_slice(&(seg.file_bytes.len() as u64).to_le_bytes()); // p_filesz
        image[ph + 40..ph + 48].copy_from_slice(&seg.memsz.to_le_bytes()); // p_memsz
        image[ph + 48..ph + 56].copy_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

        let start = offsets[i] as usize;
        image[start..start + seg.file_bytes.len()].copy_from_slice(&seg.file_bytes);
    }

    image
}

/// A representative two-segment layout matching KAOS's real linker scripts:
/// R-X text at the base of the code window, RW data+bss right after it.
fn two_segment_image() -> Vec<u8> {
    let text_vaddr = USER_CODE_BASE;
    let data_vaddr = USER_CODE_BASE + PAGE_SIZE;

    build_elf_image(
        text_vaddr, // entry at the very start of .text
        &[
            SegSpec {
                vaddr: text_vaddr,
                flags: PF_R | PF_X,
                file_bytes: vec![0x90; 16], // a few NOPs
                memsz: 16,
            },
            SegSpec {
                vaddr: data_vaddr,
                flags: PF_R | PF_W,
                file_bytes: vec![0x11; 8], // initialized .data
                memsz: 64,                 // tail 56 bytes is BSS
            },
        ],
    )
}

#[test_case]
fn test_valid_two_segment_image_parses() {
    let image = two_segment_image();
    let parsed = parse_elf64(&image).expect("well-formed two-segment ELF must parse");

    assert!(
        parsed.entry == USER_CODE_BASE,
        "entry point must round-trip exactly"
    );
    assert!(
        parsed.segments.len() == 2,
        "both PT_LOAD segments must be returned"
    );

    let text = &parsed.segments[0];
    assert!(text.executable(), "text segment must be executable");
    assert!(!text.writable(), "text segment must not be writable");
    assert!(
        text.page_count() == 1,
        "16-byte text segment must round up to exactly one page"
    );

    let data = &parsed.segments[1];
    assert!(!data.executable(), "data segment must not be executable");
    assert!(data.writable(), "data segment must be writable");
    assert!(
        data.memsz == 64 && data.filesz == 8,
        "data segment must preserve filesz/memsz (BSS tail = memsz - filesz)"
    );
}

#[test_case]
fn test_bad_magic_rejected() {
    let mut image = two_segment_image();
    image[0] = 0x00;
    assert!(
        parse_elf64(&image).err() == Some(ElfError::BadMagic),
        "corrupted magic must be rejected as BadMagic"
    );
}

#[test_case]
fn test_wrong_class_rejected() {
    let mut image = two_segment_image();
    image[4] = 1; // ELFCLASS32
    assert!(
        parse_elf64(&image).err() == Some(ElfError::NotClass64),
        "32-bit class must be rejected as NotClass64"
    );
}

#[test_case]
fn test_wrong_machine_rejected() {
    let mut image = two_segment_image();
    image[18..20].copy_from_slice(&3u16.to_le_bytes()); // EM_386
    assert!(
        parse_elf64(&image).err() == Some(ElfError::NotX86_64),
        "non-x86_64 machine must be rejected as NotX86_64"
    );
}

#[test_case]
fn test_wrong_type_rejected() {
    let mut image = two_segment_image();
    image[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    assert!(
        parse_elf64(&image).err() == Some(ElfError::NotExecutable),
        "ET_DYN must be rejected as NotExecutable (static ET_EXEC only)"
    );
}

#[test_case]
fn test_too_short_for_header_rejected() {
    let image = vec![0x7Fu8, b'E', b'L', b'F'];
    assert!(
        parse_elf64(&image).err() == Some(ElfError::TooShortForHeader),
        "an image shorter than the ELF64 header must be rejected"
    );
}

#[test_case]
fn test_filesz_exceeds_memsz_rejected() {
    let image = build_elf_image(
        USER_CODE_BASE,
        &[SegSpec {
            vaddr: USER_CODE_BASE,
            flags: PF_R | PF_X,
            file_bytes: vec![0x90; 32],
            memsz: 16, // memsz < filesz: invalid
        }],
    );
    assert!(
        parse_elf64(&image).err() == Some(ElfError::SegmentFileszExceedsMemsz { index: 0 }),
        "p_filesz > p_memsz must be rejected"
    );
}

#[test_case]
fn test_segment_outside_code_window_rejected() {
    // Far below USER_CODE_BASE — clearly outside the user code window.
    let image = build_elf_image(
        0x1000,
        &[SegSpec {
            vaddr: 0x1000,
            flags: PF_R | PF_X,
            file_bytes: vec![0x90; 16],
            memsz: 16,
        }],
    );
    assert!(
        parse_elf64(&image).err() == Some(ElfError::SegmentOutsideCodeWindow { index: 0 }),
        "a segment outside [USER_CODE_BASE, USER_CODE_END) must be rejected"
    );
}

#[test_case]
fn test_misaligned_vaddr_rejected() {
    let image = build_elf_image(
        USER_CODE_BASE + 1,
        &[SegSpec {
            vaddr: USER_CODE_BASE + 1, // not page-aligned
            flags: PF_R | PF_X,
            file_bytes: vec![0x90; 16],
            memsz: 16,
        }],
    );
    assert!(
        parse_elf64(&image).err() == Some(ElfError::SegmentOutsideCodeWindow { index: 0 }),
        "a non-page-aligned p_vaddr must be rejected"
    );
}

#[test_case]
fn test_overlapping_segments_rejected() {
    let image = build_elf_image(
        USER_CODE_BASE,
        &[
            SegSpec {
                vaddr: USER_CODE_BASE,
                flags: PF_R | PF_X,
                file_bytes: vec![0x90; 16],
                memsz: PAGE_SIZE + 1, // spills one byte into the next page
            },
            SegSpec {
                vaddr: USER_CODE_BASE + PAGE_SIZE, // starts inside the spillover
                flags: PF_R | PF_W,
                file_bytes: vec![0x11; 8],
                memsz: 64,
            },
        ],
    );
    assert!(
        parse_elf64(&image).err()
            == Some(ElfError::SegmentsOverlap {
                first: 0,
                second: 1
            }),
        "page-rounded overlapping segments must be rejected"
    );
}

#[test_case]
fn test_entry_outside_executable_segment_rejected() {
    // Entry point points into the data segment instead of the text segment.
    let text_vaddr = USER_CODE_BASE;
    let data_vaddr = USER_CODE_BASE + PAGE_SIZE;
    let image = build_elf_image(
        data_vaddr,
        &[
            SegSpec {
                vaddr: text_vaddr,
                flags: PF_R | PF_X,
                file_bytes: vec![0x90; 16],
                memsz: 16,
            },
            SegSpec {
                vaddr: data_vaddr,
                flags: PF_R | PF_W,
                file_bytes: vec![0x11; 8],
                memsz: 64,
            },
        ],
    );
    assert!(
        parse_elf64(&image).err() == Some(ElfError::EntryNotExecutable),
        "an entry point inside a non-executable segment must be rejected"
    );
}

#[test_case]
fn test_no_load_segments_rejected() {
    // phnum = 0: header-only image with no program headers at all.
    let image = build_elf_image(USER_CODE_BASE, &[]);
    assert!(
        parse_elf64(&image).err() == Some(ElfError::NoLoadSegments),
        "an image with zero PT_LOAD segments must be rejected"
    );
}
