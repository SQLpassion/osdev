//! Minimal `no_std` ELF64 reader for the static `ET_EXEC` user-program loader.
//!
//! This is deliberately narrow: KAOS only needs to map a statically linked
//! x86_64 executable into a fresh user address space, so this module reads
//! just the ELF64 file header and the `PT_LOAD` program headers. There is no
//! support for relocations, dynamic linking (`PT_INTERP`/`PT_DYNAMIC`),
//! section headers, or symbol tables — none of that is needed to map a
//! statically linked `ET_EXEC` binary, so it is intentionally out of scope.
//!
//! No external crate is used (`goblin` requires `std`); the header layouts
//! below are the fixed-size ELF64 structures from the System V ABI spec.

use alloc::vec::Vec;

use crate::memory::vmm::{USER_CODE_BASE, USER_CODE_END};

/// Size in bytes of the fixed ELF64 file header (`Elf64_Ehdr`).
const EHDR_SIZE: usize = 64;

/// Size in bytes of one ELF64 program header entry (`Elf64_Phdr`).
const PHDR_SIZE: usize = 56;

/// `e_ident[EI_CLASS]` value for 64-bit objects.
const ELFCLASS64: u8 = 2;

/// `e_ident[EI_DATA]` value for little-endian objects.
const ELFDATA2LSB: u8 = 1;

/// `e_ident[EI_VERSION]` / `e_version` value for the current ELF spec.
const EV_CURRENT: u8 = 1;

/// `e_type` value for a static executable (as opposed to `ET_DYN`/`ET_REL`).
const ET_EXEC: u16 = 2;

/// `e_machine` value for x86_64.
const EM_X86_64: u16 = 62;

/// `p_type` value marking a loadable segment.
const PT_LOAD: u32 = 1;

/// `p_flags` bit: segment is executable.
pub const PF_X: u32 = 1 << 0;
/// `p_flags` bit: segment is writable.
pub const PF_W: u32 = 1 << 1;
/// `p_flags` bit: segment is readable.
pub const PF_R: u32 = 1 << 2;

/// Hard cap on the number of program-header entries this parser will walk.
///
/// A well-formed KAOS user program has two `PT_LOAD` segments (text+rodata,
/// data+bss). This bound exists purely to keep a corrupt or hostile
/// `e_phnum` from turning the header walk into an unbounded loop over
/// attacker-controlled input; it is not a meaningful program limit.
const MAX_PROGRAM_HEADERS: u16 = 32;

/// Rejection reasons for a malformed or unsupported ELF64 image.
///
/// Every variant maps to a concrete, checked validation failure — see the
/// call sites in [`parse_elf64`] for what triggers each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    /// Image is shorter than a fixed ELF64 file header.
    TooShortForHeader,
    /// `e_ident` magic bytes are not `0x7F 'E' 'L' 'F'`.
    BadMagic,
    /// `e_ident[EI_CLASS]` is not `ELFCLASS64`.
    NotClass64,
    /// `e_ident[EI_DATA]` is not `ELFDATA2LSB`.
    NotLittleEndian,
    /// `e_ident[EI_VERSION]` or `e_version` is not `EV_CURRENT`.
    BadVersion,
    /// `e_type` is not `ET_EXEC` (dynamic/relocatable objects are unsupported).
    NotExecutable,
    /// `e_machine` is not `EM_X86_64`.
    NotX86_64,
    /// `e_phnum` exceeds [`MAX_PROGRAM_HEADERS`].
    TooManyProgramHeaders,
    /// `e_phentsize` is not the fixed ELF64 program-header size.
    BadProgramHeaderSize,
    /// The program header table `[e_phoff, e_phoff + e_phnum*e_phentsize)` does
    /// not fit inside the image.
    ProgramHeaderTableOutOfBounds,
    /// No `PT_LOAD` segment was found.
    NoLoadSegments,
    /// A `PT_LOAD` segment has `p_filesz > p_memsz`.
    SegmentFileszExceedsMemsz { index: usize },
    /// A `PT_LOAD` segment's file range `[p_offset, p_offset + p_filesz)` does
    /// not fit inside the image.
    SegmentFileRangeOutOfBounds { index: usize },
    /// A `PT_LOAD` segment's `p_vaddr` is not page-aligned, or its mapped
    /// range `[p_vaddr, p_vaddr + p_memsz)` (rounded to page granularity)
    /// does not lie entirely inside the user code window.
    SegmentOutsideCodeWindow { index: usize },
    /// Two `PT_LOAD` segments' page-rounded virtual ranges overlap.
    SegmentsOverlap { first: usize, second: usize },
    /// `e_entry` does not fall inside any executable `PT_LOAD` segment.
    EntryNotExecutable,
}

/// One validated `PT_LOAD` segment, ready for the loader to map.
#[derive(Debug, Clone, Copy)]
pub struct ElfSegment {
    /// Virtual address the segment must be mapped at (page-aligned).
    pub vaddr: u64,
    /// Byte offset into the ELF image where the segment's file content starts.
    pub offset: u64,
    /// Number of bytes to copy from the image (the rest of `memsz` is BSS/zero-fill).
    pub filesz: u64,
    /// Total mapped size in memory; always `>= filesz`.
    pub memsz: u64,
    /// Raw `p_flags` (test with [`PF_R`]/[`PF_W`]/[`PF_X`]).
    pub flags: u32,
}

impl ElfSegment {
    /// Whether the segment must end up writable in the final page permissions.
    #[inline]
    pub fn writable(&self) -> bool {
        self.flags & PF_W != 0
    }

    /// Whether the segment must end up executable (i.e. NOT no-execute) in
    /// the final page permissions.
    #[inline]
    pub fn executable(&self) -> bool {
        self.flags & PF_X != 0
    }

    /// Page-aligned end address of this segment's mapped range (exclusive).
    #[inline]
    pub fn mapped_end(&self) -> u64 {
        page_align_up(self.vaddr + self.memsz)
    }

    /// Number of 4 KiB pages this segment's mapped range occupies.
    #[inline]
    pub fn page_count(&self) -> usize {
        ((self.mapped_end() - self.vaddr) / crate::arch::constants::PAGE_SIZE_U64) as usize
    }
}

/// A parsed, fully validated static executable ready for per-segment mapping.
pub struct ElfImage {
    /// Initial RIP (`e_entry`), already validated to fall inside an
    /// executable `PT_LOAD` segment.
    pub entry: u64,
    /// Validated, non-overlapping `PT_LOAD` segments in file order.
    pub segments: Vec<ElfSegment>,
}

/// Rounds `addr` up to the next 4 KiB page boundary.
#[inline]
fn page_align_up(addr: u64) -> u64 {
    let page_size = crate::arch::constants::PAGE_SIZE_U64;
    (addr + page_size - 1) & !(page_size - 1)
}

/// Reads a little-endian `u16` at `offset` in `data`.
///
/// Caller must ensure `offset + 2 <= data.len()`.
#[inline]
fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

/// Reads a little-endian `u32` at `offset` in `data`.
///
/// Caller must ensure `offset + 4 <= data.len()`.
#[inline]
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Reads a little-endian `u64` at `offset` in `data`.
///
/// Caller must ensure `offset + 8 <= data.len()`.
#[inline]
fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

/// Parses and fully validates a static `ET_EXEC` x86_64 ELF64 image.
///
/// On success, every returned [`ElfSegment`] is guaranteed to:
/// - be page-aligned (`vaddr % 4096 == 0`),
/// - lie entirely inside `[USER_CODE_BASE, USER_CODE_END)` once rounded up to
///   page granularity,
/// - not overlap any other returned segment's page-rounded range,
/// - satisfy `filesz <= memsz`, with `[offset, offset + filesz)` inside the image.
///
/// `entry` is guaranteed to fall inside an executable (`PF_X`) segment.
pub fn parse_elf64(image: &[u8]) -> Result<ElfImage, ElfError> {
    if image.len() < EHDR_SIZE {
        return Err(ElfError::TooShortForHeader);
    }

    if image[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Err(ElfError::BadMagic);
    }
    if image[4] != ELFCLASS64 {
        return Err(ElfError::NotClass64);
    }
    if image[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }
    if image[6] != EV_CURRENT {
        return Err(ElfError::BadVersion);
    }

    let e_type = read_u16(image, 16);
    if e_type != ET_EXEC {
        return Err(ElfError::NotExecutable);
    }

    let e_machine = read_u16(image, 18);
    if e_machine != EM_X86_64 {
        return Err(ElfError::NotX86_64);
    }

    let e_version = read_u32(image, 20);
    if e_version != EV_CURRENT as u32 {
        return Err(ElfError::BadVersion);
    }

    let e_entry = read_u64(image, 24);
    let e_phoff = read_u64(image, 32);
    let e_phentsize = read_u16(image, 54);
    let e_phnum = read_u16(image, 56);

    if e_phentsize as usize != PHDR_SIZE {
        return Err(ElfError::BadProgramHeaderSize);
    }
    if e_phnum > MAX_PROGRAM_HEADERS {
        return Err(ElfError::TooManyProgramHeaders);
    }

    // Bounds-check the whole program header table up front so every entry
    // read below is guaranteed in-range without a per-entry length check.
    let phdr_table_len = e_phnum as u64 * e_phentsize as u64;
    let phdr_table_end = e_phoff
        .checked_add(phdr_table_len)
        .ok_or(ElfError::ProgramHeaderTableOutOfBounds)?;
    if phdr_table_end > image.len() as u64 {
        return Err(ElfError::ProgramHeaderTableOutOfBounds);
    }

    let mut segments: Vec<ElfSegment> = Vec::new();

    for i in 0..e_phnum as usize {
        let phdr_offset = (e_phoff as usize) + i * PHDR_SIZE;
        let p_type = read_u32(image, phdr_offset);
        if p_type != PT_LOAD {
            continue;
        }

        let p_flags = read_u32(image, phdr_offset + 4);
        let p_offset = read_u64(image, phdr_offset + 8);
        let p_vaddr = read_u64(image, phdr_offset + 16);
        // p_paddr (offset + 24) is intentionally ignored: KAOS user programs
        // are position-mapped by virtual address only.
        let p_filesz = read_u64(image, phdr_offset + 32);
        let p_memsz = read_u64(image, phdr_offset + 40);
        // p_align (offset + 48) is not consulted: KAOS only ever creates 4 KiB
        // mappings, so segment placement is validated against 4 KiB alignment
        // directly instead of trusting the linker-supplied alignment hint.

        if p_filesz > p_memsz {
            return Err(ElfError::SegmentFileszExceedsMemsz { index: i });
        }

        let file_end = p_offset
            .checked_add(p_filesz)
            .ok_or(ElfError::SegmentFileRangeOutOfBounds { index: i })?;
        if file_end > image.len() as u64 {
            return Err(ElfError::SegmentFileRangeOutOfBounds { index: i });
        }

        let page_size = crate::arch::constants::PAGE_SIZE_U64;
        if !p_vaddr.is_multiple_of(page_size) {
            return Err(ElfError::SegmentOutsideCodeWindow { index: i });
        }

        let mapped_end = p_vaddr
            .checked_add(p_memsz)
            .map(page_align_up)
            .ok_or(ElfError::SegmentOutsideCodeWindow { index: i })?;
        if p_vaddr < USER_CODE_BASE || mapped_end > USER_CODE_END {
            return Err(ElfError::SegmentOutsideCodeWindow { index: i });
        }

        segments.push(ElfSegment {
            vaddr: p_vaddr,
            offset: p_offset,
            filesz: p_filesz,
            memsz: p_memsz,
            flags: p_flags,
        });
    }

    if segments.is_empty() {
        return Err(ElfError::NoLoadSegments);
    }

    // Pairwise overlap check on page-rounded ranges. Segment counts are tiny
    // (2 in practice, capped at MAX_PROGRAM_HEADERS), so O(n^2) is fine here.
    for a in 0..segments.len() {
        for b in (a + 1)..segments.len() {
            let (seg_a, seg_b) = (&segments[a], &segments[b]);
            let overlap = seg_a.vaddr < seg_b.mapped_end() && seg_b.vaddr < seg_a.mapped_end();
            if overlap {
                return Err(ElfError::SegmentsOverlap {
                    first: a,
                    second: b,
                });
            }
        }
    }

    let entry_is_executable = segments
        .iter()
        .any(|s| s.executable() && e_entry >= s.vaddr && e_entry < s.vaddr + s.memsz);
    if !entry_is_executable {
        return Err(ElfError::EntryNotExecutable);
    }

    Ok(ElfImage {
        entry: e_entry,
        segments,
    })
}
