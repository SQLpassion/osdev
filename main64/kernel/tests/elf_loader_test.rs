//! ELF loader integration tests: `process::map_program_image_into_user_address_space`
//! end-to-end for a synthetic two-segment ELF64 image.
//!
//! Pins the per-segment mapping contract the loader requires: a R-X text
//! segment, a RW- data segment with a zero-filled BSS tail, and `entry_rip`
//! sourced from `e_entry` rather than the legacy fixed
//! `USER_PROGRAM_ENTRY_RIP` constant.

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
use kaos_kernel::process;

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
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

struct SegSpec {
    vaddr: u64,
    flags: u32,
    file_bytes: Vec<u8>,
    memsz: u64,
}

/// Builds a minimal well-formed ELF64 `ET_EXEC`/`EM_X86_64` image. Mirrors the
/// builder in `elf_test.rs` (each `kernel/tests/*.rs` file is a standalone
/// test binary in this harness, so there is no shared support module to pull
/// this from).
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
        image[ph + 8..ph + 16].copy_from_slice(&offsets[i].to_le_bytes());
        image[ph + 16..ph + 24].copy_from_slice(&seg.vaddr.to_le_bytes());
        image[ph + 24..ph + 32].copy_from_slice(&seg.vaddr.to_le_bytes());
        image[ph + 32..ph + 40].copy_from_slice(&(seg.file_bytes.len() as u64).to_le_bytes());
        image[ph + 40..ph + 48].copy_from_slice(&seg.memsz.to_le_bytes());
        image[ph + 48..ph + 56].copy_from_slice(&PAGE_SIZE.to_le_bytes());

        let start = offsets[i] as usize;
        image[start..start + seg.file_bytes.len()].copy_from_slice(&seg.file_bytes);
    }

    image
}

/// Two-segment layout: R-X text (entry 4 bytes in, to prove `e_entry` -- not
/// segment start -- drives `entry_rip`), RW- data with a BSS tail.
fn two_segment_image() -> (Vec<u8>, u64, u64, Vec<u8>, Vec<u8>) {
    let text_vaddr = USER_CODE_BASE;
    let data_vaddr = USER_CODE_BASE + PAGE_SIZE;
    let entry = text_vaddr + 4;

    let text_bytes: Vec<u8> = vec![0x90, 0x90, 0x90, 0x90, 0xF4, 0xF4, 0xF4, 0xF4];
    let data_bytes: Vec<u8> = vec![0xAA, 0xBB, 0xCC, 0xDD];

    let image = build_elf_image(
        entry,
        &[
            SegSpec {
                vaddr: text_vaddr,
                flags: PF_R | PF_X,
                file_bytes: text_bytes.clone(),
                memsz: text_bytes.len() as u64,
            },
            SegSpec {
                vaddr: data_vaddr,
                flags: PF_R | PF_W,
                file_bytes: data_bytes.clone(),
                memsz: 32, // tail (32 - 4) = 28 bytes must end up zeroed (BSS)
            },
        ],
    );

    (image, text_vaddr, data_vaddr, text_bytes, data_bytes)
}

#[test_case]
fn test_elf_program_maps_with_entry_from_e_entry() {
    let (image, text_vaddr, _data_vaddr, _text_bytes, _data_bytes) = two_segment_image();
    let expected_entry = text_vaddr + 4;

    let loaded = process::map_program_image_into_user_address_space(&image)
        .expect("well-formed two-segment ELF must map");

    assert!(loaded.cr3 != 0, "mapped ELF program must return non-zero CR3");
    assert!(
        loaded.entry_rip == expected_entry,
        "entry_rip must come from e_entry, not the fixed flat-path constant"
    );
    assert!(
        loaded.entry_rip != text_vaddr,
        "test fixture deliberately offsets e_entry from the segment base to \
         prove entry_rip isn't just USER_PROGRAM_ENTRY_RIP in disguise"
    );
    assert!(
        loaded.code_page_count == 2,
        "one text page + one data page must be reflected in code_page_count"
    );
}

#[test_case]
fn test_elf_text_segment_is_read_only_executable() {
    let (image, text_vaddr, _data_vaddr, text_bytes, _data_bytes) = two_segment_image();
    let loaded = process::map_program_image_into_user_address_space(&image)
        .expect("well-formed two-segment ELF must map");

    vmm::with_address_space(loaded.cr3, || {
        let flags = vmm::debug_mapping_flags_for_va(text_vaddr)
            .expect("mapped text page must expose mapping flags");
        let (_pml4_u, _pdp_u, _pd_u, pt_u, pt_w) = flags;
        assert!(pt_u, "text page must be user-accessible");
        assert!(!pt_w, "text page must not be writable (PF_W not set)");

        let no_execute = vmm::debug_no_execute_flag_for_va(text_vaddr)
            .expect("mapped text page must expose NX bit");
        assert!(!no_execute, "text page must be executable (PF_X set)");

        // SAFETY:
        // - Loader mapped the text segment's page(s) in this address space and
        //   copied `text_bytes` into it.
        // - Reading `text_bytes.len()` bytes from `text_vaddr` is valid.
        unsafe {
            let base = text_vaddr as *const u8;
            for (idx, expected) in text_bytes.iter().enumerate() {
                let actual = core::ptr::read_volatile(base.add(idx));
                assert!(
                    actual == *expected,
                    "text segment byte {} must match source image",
                    idx
                );
            }
        }
    });
}

#[test_case]
fn test_elf_data_segment_is_writable_non_executable_with_zeroed_bss() {
    let (image, _text_vaddr, data_vaddr, _text_bytes, data_bytes) = two_segment_image();
    let loaded = process::map_program_image_into_user_address_space(&image)
        .expect("well-formed two-segment ELF must map");

    vmm::with_address_space(loaded.cr3, || {
        let flags = vmm::debug_mapping_flags_for_va(data_vaddr)
            .expect("mapped data page must expose mapping flags");
        let (_pml4_u, _pdp_u, _pd_u, pt_u, pt_w) = flags;
        assert!(pt_u, "data page must be user-accessible");
        assert!(pt_w, "data page must be writable (PF_W set)");

        let no_execute = vmm::debug_no_execute_flag_for_va(data_vaddr)
            .expect("mapped data page must expose NX bit");
        assert!(no_execute, "data page must be non-executable (PF_X not set)");

        // SAFETY:
        // - Loader mapped the data segment's page in this address space and
        //   copied `data_bytes`, zero-filling the BSS tail.
        // - Reading 32 bytes from `data_vaddr` (this segment's memsz) is valid.
        unsafe {
            let base = data_vaddr as *const u8;
            for (idx, expected) in data_bytes.iter().enumerate() {
                let actual = core::ptr::read_volatile(base.add(idx));
                assert!(
                    actual == *expected,
                    "data segment byte {} must match source image",
                    idx
                );
            }
            for idx in data_bytes.len()..32 {
                let actual = core::ptr::read_volatile(base.add(idx));
                assert!(actual == 0, "BSS tail byte {} must be zero-filled", idx);
            }
        }
    });
}
