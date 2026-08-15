# The ELF Program Loader

This document explains two things, in order:

1. **What ELF is.** A from-scratch introduction to the ELF (Executable and
   Linkable Format) executable file format — what a "section" is, what a
   "segment" is, why both exist, and what an operating system actually has to
   do with a file in this format to run it. No prior knowledge of object-file
   formats is assumed.
2. **How KAOS uses it.** A detailed, code-level walkthrough of
   `main64/kernel/src/process/elf.rs` and `main64/kernel/src/process/loader.rs`
   — the parser and loader that turn an ELF64 file sitting on the FAT32 disk
   into a running ring-3 task.

Before this loader existed, KAOS ran user programs as raw flat binaries
(`objcopy -O binary` output — no headers, no segments, just bytes mapped
starting at a fixed address). That approach has a fundamental limitation
around zero-initialized data (explained in full in §1.4–1.5) that eventually
became a real, reproducible kernel panic. This document doesn't dwell on
that history — it documents the ELF-based design that replaced it, as it
exists today.

---

## Part 1: What is ELF?

### 1.1 The problem a file format solves

When you compile a C or Rust program, the compiler doesn't produce machine
code that can run immediately. It produces machine code for *pieces* of the
program (functions, string constants, global variables) without knowing yet
at which memory addresses those pieces will end up. The **linker** then
combines all those pieces — potentially from several compiled files — into
one file, decides on final addresses, and patches up all the places that
referred to "wherever this ends up" (this patching is called
**relocation**).

The output of that linking step needs a file format that can answer two
different questions for two different audiences:

- For **other tools** (debuggers, linkers, disassemblers): "Where is the
  symbol table? Where is the debug information? Which named region of the
  file corresponds to which function?"
- For **the operating system**, at the moment it wants to actually run the
  program: "Which *bytes* of this file need to end up at which *virtual
  addresses* in memory, with which *permissions* (readable / writable /
  executable), before I can jump to the entry point?"

ELF answers both questions in the same file, using two independent, parallel
descriptions of the same underlying bytes: **sections** (for the first
audience) and **segments** (for the second). This split is the single most
important idea to understand about ELF, and it is also the idea that KAOS's
loader is built entirely around — so it's worth spending time on before
looking at any code.

### 1.2 Sections: the linker's and compiler's view

A **section** is a named, typed, contiguous run of bytes in the file, used
by build tools. Typical sections in a compiled program:

| Section    | Contents                                                | Occupies file space? |
|------------|----------------------------------------------------------|-----------------------|
| `.text`    | Machine code (the actual CPU instructions)               | Yes |
| `.rodata`  | Read-only constants (string literals, const tables)       | Yes |
| `.data`    | Global/static variables that have a **non-zero** initial value | Yes |
| `.bss`     | Global/static variables that are **zero-initialized**     | **No** |
| `.symtab`  | Symbol table (function/variable names ↔ addresses), used by linkers/debuggers | Yes, but usually stripped from shipped binaries |
| `.debug_*` | Debug info (DWARF), used by debuggers                     | Yes, but usually stripped |

The single most important thing about this list is the `.bss` row: **it
takes up no space in the file at all.** If you write:

```rust
static mut COUNTER: u64 = 0;
static mut BUFFER: [u8; 4096] = [0; 4096];
```

there is no reason to store 4096+ zero bytes on disk — the linker just
records "this section is 4104 bytes of zeros" as a *size*, without any
matching bytes in the file. This is a deliberate and universal space
optimization; every linker on every platform does this. It has one
unavoidable consequence for anyone who wants to load ELF files: **the file's
byte length is not the same as the program's in-memory footprint.** Whatever
loads the program has to know how much *extra*, file-absent, zero-filled
space to reserve past the end of the file content. This single fact is the
reason segments (§1.3) look the way they do, and it is also — see §1.5 — the
exact bug that motivated this rewrite in KAOS.

Sections exist purely as a convenience for build-time and debug-time tools.
**The kernel loader described in this document never looks at section
headers at all** — no `.text`, `.data`, `.symtab`, or anything else is ever
read by KAOS at load time. This is normal: on Linux, the kernel's own ELF
loader (`binfmt_elf.c`) doesn't consult section headers either. Sections are
for humans and tools working *before* the program runs; segments (next) are
for the loader that runs it.

### 1.3 Segments: the loader's view

A **segment** is a *different* grouping of the exact same underlying bytes,
described by the **program header table**. Where a section says "these
bytes are named `.text`", a segment says "these bytes need to be placed at
virtual address X, memory address range of size Y, with permissions Z,
before this program can execute." Segments are coarser than sections: a
typical statically linked executable has only one or two segments of the
type that matters for execution (`PT_LOAD`, below), each one bundling
several sections together.

There are several segment types (`p_type` in the program header), but only
one matters for a simple static executable:

- **`PT_LOAD`** — "map these file bytes into memory at this address, with
  these permissions." This is the only segment type KAOS's loader
  understands or needs; see §1.6 for why the others (`PT_INTERP`,
  `PT_DYNAMIC`, `PT_TLS`, `PT_GNU_STACK`, ...) don't apply to KAOS user
  programs.

Each `PT_LOAD` program header entry carries exactly the fields an OS loader
needs to answer "where, how much, and with what permissions":

| Field       | Meaning |
|-------------|---------|
| `p_vaddr`   | The virtual address this segment must be mapped at. |
| `p_offset`  | The byte offset **inside the file** where this segment's content starts. |
| `p_filesz`  | How many bytes of **actual file content** this segment has. |
| `p_memsz`   | How many bytes this segment occupies **in memory** once loaded. Always `>= p_filesz`. |
| `p_flags`   | Permission bits: `PF_R` (readable), `PF_W` (writable), `PF_X` (executable). |
| `p_align`   | Alignment hint (normally the page size, 4 KiB, on x86-64). |
| `p_paddr`   | Physical address hint — meaningless for a paged OS with virtual memory; ignored. |

The relationship `p_memsz >= p_filesz` is exactly how ELF represents `.bss`
without wasting file space: `p_filesz` bytes get copied verbatim from the
file, and the remaining `p_memsz - p_filesz` bytes are supposed to be
**zero-filled by the loader**, not read from disk. A segment can therefore
contain a run of "real" initialized data (`.data`, mapped by `p_filesz`)
immediately followed by a run of implicit zero bytes (`.bss`, covered only
by `p_memsz`) — that's exactly how the linker packs `.data` and `.bss` into
one `RW-` `PT_LOAD` segment (see §1.4 and, for KAOS's own linker scripts,
§3.9).

A well-formed statically linked executable typically has exactly two
`PT_LOAD` segments:

```text
Segment 1 ("text"): p_flags = R-X   (readable + executable, NOT writable)
    contains: .text (code), .rodata (constants)
    p_filesz == p_memsz   (code has no equivalent of BSS — there's nothing to zero-fill)

Segment 2 ("data"): p_flags = RW-   (readable + writable, NOT executable)
    contains: .data (initialized globals), .bss (zero-initialized globals)
    p_filesz  = size of .data content
    p_memsz   = size of .data + .bss  (the extra tail is zero-filled by the loader)
```

Splitting code and data into two segments with different permissions is not
an accident — it's the entire point of having per-segment permissions at
all: it lets the OS enforce that a program's code cannot be overwritten
(`R-X`, not `W`) and that its writable data cannot be executed (`RW-`, not
`X`). This is the classic **W^X** ("write XOR execute") security property,
and it is only possible because the loader maps each segment with its own
`p_flags` instead of mapping the whole program as one uniformly-permissioned
blob. §2 shows exactly how KAOS enforces this.

### 1.4 A concrete before/after picture

Take this Rust snippet:

```rust
static GREETING: &str = "Hello";     // .rodata — read-only constant
static mut COUNTER: u64 = 0;          // .bss — zero-initialized
fn main() { ... }                     // .text — code
```

On disk (`p_filesz` bytes only — no zeros stored for `COUNTER`):

```text
file offset 0x0000  ELF header + program headers
file offset 0x1000  [ .text bytes ] [ .rodata bytes: "Hello\0" ]      ← Segment 1 (R-X)
file offset 0x2000  [ .data bytes: (none in this example) ]           ← Segment 2 (RW-), p_filesz ends here
                     (file ends — no bytes stored for COUNTER)
```

In memory, after the loader has done its job (`p_memsz` bytes, tail
zero-filled):

```text
vaddr 0x...1000   Segment 1, mapped R-X:  .text, .rodata           (p_filesz == p_memsz)
vaddr 0x...2000   Segment 2, mapped RW-:  .data (from file)
vaddr 0x...2008   Segment 2, continued:   COUNTER = 0x0000000000000000  ← zero-filled by loader, not read from file
```

If a loader naively computed "how many pages do I need" from the *file
length* instead of from `p_memsz`, the `COUNTER` page would never get
mapped at all — and the first write to it would page-fault. This is
exactly the bug KAOS's old flat-binary loader had (it computed
`ceil(file_length / PAGE_SIZE)` pages, since a flat binary has no `p_memsz`
to consult at all — see §2.9 for how it was worked around), and it is why
`p_memsz` (not file length) drives every allocation decision in the loader
described in §2 below.

### 1.5 The rest of the ELF file (and why KAOS ignores most of it)

An ELF64 file is, top to bottom:

```text
+-------------------------------------------+
| ELF header (Elf64_Ehdr, fixed 64 bytes)   |  <- identifies "this is ELF64", architecture, entry point,
+-------------------------------------------+     and *where* the next two tables live
| Program header table (Elf64_Phdr[])       |  <- one entry per segment (used by the OS loader)
+-------------------------------------------+
| ... actual section content (.text, ...) ...|
+-------------------------------------------+
| Section header table (Elf64_Shdr[])       |  <- one entry per section (used by linkers/debuggers)
+-------------------------------------------+
```

The ELF header (`Elf64_Ehdr`) is a fixed 64-byte structure at file offset 0.
The fields KAOS's parser reads from it are:

| Offset | Field         | Meaning |
|--------|---------------|---------|
| 0x00   | `e_ident[0..4]` | Magic bytes: must be `0x7F 'E' 'L' 'F'`. |
| 0x04   | `e_ident[EI_CLASS]` | `1` = 32-bit, `2` = 64-bit. KAOS requires `2`. |
| 0x05   | `e_ident[EI_DATA]`  | `1` = little-endian, `2` = big-endian. KAOS requires `1` (x86-64 is little-endian). |
| 0x06   | `e_ident[EI_VERSION]` | Must be `1` (`EV_CURRENT`; there has only ever been one ELF version). |
| 0x10   | `e_type`      | `1`=`ET_REL` (unlinked object), `2`=`ET_EXEC` (static executable), `3`=`ET_DYN` (shared object / PIE), `4`=`ET_CORE` (core dump). KAOS requires `ET_EXEC`. |
| 0x12   | `e_machine`   | CPU architecture. KAOS requires `62` (`EM_X86_64`). |
| 0x14   | `e_version`   | Must equal `EV_CURRENT` again (redundant with the `e_ident` byte, kept for historical ABI reasons). |
| 0x18   | `e_entry`     | Virtual address of the very first instruction to execute. |
| 0x20   | `e_phoff`     | File byte offset where the program header table starts. |
| 0x36   | `e_phentsize` | Size in bytes of **one** program header entry (56 for ELF64). |
| 0x38   | `e_phnum`     | **Number** of program header entries. |

Everything below this point in the file — the section header table
(`e_shoff`/`e_shentsize`/`e_shnum`), the symbol table, string tables, debug
info — is read by KAOS **never**. It is entirely a linker/debugger/toolchain
concern. This is why the build pipeline (§3.10) is allowed to strip debug
info with `llvm-strip --strip-debug` before shipping a binary to the disk
image: that only removes section-header-described content, and the loader
was never going to look at it anyway.

### 1.6 Static vs. dynamic executables, and why KAOS only needs `ET_EXEC`

`e_type` distinguishes several kinds of ELF files. The two relevant to
"things that can run" are:

- **`ET_EXEC`** (static executable): every address in the file is already
  final. The linker chose one fixed load address at link time (KAOS uses
  `0x0000_7000_0000_0000`, see §2.3), and the loader's only job is: read
  program headers, copy bytes to those addresses, jump to `e_entry`. No
  runtime relocation processing needed.
- **`ET_DYN`** (shared object / position-independent executable): the file
  can be loaded at *any* base address, chosen by the loader at run time
  (this is what makes ASLR — Address Space Layout Randomization — possible).
  This requires the loader to process a relocation table, patching every
  address-dependent instruction/data reference to account for wherever the
  file actually ended up. It typically also requires a **dynamic linker**
  (`PT_INTERP` names something like `/lib64/ld-linux-x86-64.so.2`) to
  resolve symbols against shared libraries at load time.

KAOS user programs are built and shipped as `ET_EXEC` only. There is
exactly one process on the system at a time occupying the fixed user-code
window (`USER_CODE_BASE`, §2.3), each with its own address space (its own
page tables / CR3), so there's no address-space collision to avoid by
randomizing load addresses, and there's no dynamic linker to write. This
also means the following ELF concepts are **explicitly out of scope**, and
KAOS's parser will reject a file that needs them:

- **Dynamic linking / relocations** (`PT_DYNAMIC`, `PT_INTERP`) — not
  needed for static `ET_EXEC` binaries; `parse_elf64` rejects any `e_type`
  other than `ET_EXEC` outright (§2.4).
- **PIE / ASLR** — requires `ET_DYN` plus a base-address picker; not
  implemented.
- **Thread-local storage** (`PT_TLS`) — no userspace threads exist yet.
- **`PT_GNU_STACK`** — a hint about whether the stack should be executable;
  irrelevant because KAOS's kernel creates the user stack itself and always
  maps it non-executable (§2.7), regardless of what the ELF file says.

### 1.7 What an OS loader does, in eight steps

Putting §1.1–1.6 together, loading *any* static ELF executable — on Linux,
on KAOS, or on any other OS — is fundamentally the same eight-step
algorithm:

1. Read the ELF header. Verify the magic, class, endianness, version,
   type, and machine fields — reject anything that doesn't match what this
   CPU/OS combination can run.
2. Read the program header table (`e_phoff`, `e_phnum`, `e_phentsize`).
3. For each `PT_LOAD` entry: compute the page-aligned virtual address range
   it needs (`p_vaddr` through `p_vaddr + p_memsz`, rounded up).
4. Allocate physical memory frames to back that range.
5. Map those frames at the segment's virtual address range — initially
   writable, regardless of the segment's final permissions (you need to be
   able to write the copied bytes and the zero-fill into it first).
6. Copy `p_filesz` bytes from the file (at `p_offset`) into the mapped
   region, then zero-fill the remaining `p_memsz - p_filesz` bytes (the
   implicit BSS tail) plus any leftover space in the last page.
7. Re-protect (tighten) the mapping to the segment's real, final
   permissions, derived from `p_flags` (`PF_R`/`PF_W`/`PF_X`).
8. Set up an initial stack, then transfer control to `e_entry` in ring 3 /
   user mode.

Part 2 shows exactly where each of these eight steps lives in KAOS's source
tree, and what KAOS does differently or additionally (bounds/overlap
validation, a fixed single-process address window, an explicit rollback
path on any mid-way failure).

---

## Part 2: The KAOS ELF Loader

### 2.1 Where the code lives

| Concern | File |
|---|---|
| ELF64 parsing/validation (steps 1–2 above) | `main64/kernel/src/process/elf.rs` |
| Program loading, per-segment map/copy/protect (steps 3–7) | `main64/kernel/src/process/loader.rs` |
| Low-level page-table primitives used by the loader | `main64/kernel/src/memory/vmm/mapping.rs` |
| Fault policy for the mapped code window (post-load) | `main64/kernel/src/memory/vmm/page_fault.rs` |
| Process-level types/errors (`LoadedProgram`, `ExecError`) | `main64/kernel/src/process/types.rs` |
| User-program linker scripts (produce the two `PT_LOAD` segments) | `main64/user_programs/*/link.ld` |
| Build pipeline (compiles + ships the ELF file, no more `objcopy`) | `main64/build/helper_build_user_programs.sh` |

While the seven in-tree programs (`hello`, `readline`, `filedemo`,
`exception_test`, `shell`, `tui_app`, `kbasic`) were migrated from the old
flat-binary format one at a time, the loader briefly supported both formats
side by side — sniffing the ELF magic and falling back to the old
flat-mapping path for anything that didn't have it. That fallback (and the
flat-mapping code path itself) was removed once all seven programs shipped
as ELF; a non-ELF image is now rejected outright with
`ExecError::InvalidElfImage` (§2.4), with no fallback to any other loading
strategy.

### 2.2 End-to-end call flow

```text
exec_from_vfs("SHELL.BIN")                          [process/loader.rs]
  └─ load_program_image()                            reads raw bytes from FAT32 via the VFS
       └─ validate_program_image_len()                rejects empty / oversized images early
  └─ map_program_image_into_user_address_space()
       └─ map_elf_program_image()
            └─ elf::parse_elf64()                     [process/elf.rs]  → ElfImage { entry, segments }
            └─ vmm::clone_kernel_pml4_for_user()       fresh CR3 for this process
            └─ try_map_elf_program_image()             for each PT_LOAD segment:
                 ├─ allocate physical frames (PMM)
                 ├─ map_user_code_page(.., writable=true, executable=true)   (transient, over-permissive)
                 ├─ copy p_filesz bytes, zero-fill the rest (BSS + page tail)
                 └─ map_user_code_page(.., final p_flags)                    (tightened, final)
            └─ map one bootstrap stack page, zero it
       └─ returns LoadedProgram { cr3, entry_rip = e_entry, user_rsp, ... }
  └─ spawn_loaded_program()                            hands (cr3, entry_rip, user_rsp) to the scheduler
```

`exec_from_image()` is the same pipeline minus the VFS read — used once, at
boot, to launch the initial shell from a buffer the boot path already read
into memory.

### 2.3 The fixed address-space layout

Because every KAOS user program is a fixed-address `ET_EXEC` binary (§1.6),
the kernel can define one fixed virtual-address window that every process's
code and data must live inside, and reject at parse time any segment that
would fall outside it. These constants live in
`main64/kernel/src/memory/vmm/vmm_constants.rs`:

```text
USER_CODE_BASE  = 0x0000_7000_0000_0000     ← every program's PT_LOAD segments must live here
USER_CODE_SIZE  = 0x0020_0000                (2 MiB)
USER_CODE_END   = USER_CODE_BASE + USER_CODE_SIZE

USER_HEAP_BASE  = 0x0000_7000_1000_0000     (256 MiB heap, separate window, unrelated to ELF segments)
USER_HEAP_END   = USER_HEAP_BASE + 0x1000_0000

USER_STACK_TOP  = 0x0000_7FFF_F000_0000     ← stack grows downward from here (1 MiB region)
USER_STACK_BASE = USER_STACK_TOP - 0x0010_0000
```

Every KAOS user program's linker script (§2.9) places `. = 0x0000700000000000;`
as its very first statement — i.e. `USER_CODE_BASE` is baked into every
program at link time, not chosen dynamically. `parse_elf64` (§2.4) then
independently double-checks that every `PT_LOAD` segment's virtual range
actually falls inside `[USER_CODE_BASE, USER_CODE_END)`; a program whose
linker script or manual construction disagrees with this window is rejected
at load time rather than trusted.

### 2.4 The parser: `process/elf.rs`

`parse_elf64(image: &[u8]) -> Result<ElfImage, ElfError>` implements ELF
loader steps 1–2 (§1.7): it reads the header, validates it, reads every
program header entry, and returns a fully validated, ready-to-map
`ElfImage`:

```rust
pub struct ElfImage {
    pub entry: u64,                // e_entry, already checked to land in an executable segment
    pub segments: Vec<ElfSegment>, // validated, non-overlapping PT_LOAD segments, in file order
}

pub struct ElfSegment {
    pub vaddr: u64,    // p_vaddr   — page-aligned virtual address
    pub offset: u64,   // p_offset  — file byte offset of the segment's content
    pub filesz: u64,   // p_filesz  — bytes to copy from the file
    pub memsz: u64,    // p_memsz   — total bytes mapped in memory (>= filesz)
    pub flags: u32,    // p_flags   — PF_R / PF_W / PF_X bits
}
```

**Why "fully validated" matters.** This file is read from a FAT32 disk —
untrusted, in the sense that a corrupted disk image, a buggy build, or (in
a security context) a deliberately hostile file must never be able to
crash the kernel or corrupt kernel memory just by being `exec`'d. Every
field taken from the file is bounds-checked *before* anything is mapped or
copied, using `checked_add` (never raw `+`) wherever a malicious or
corrupted offset could otherwise overflow a `u64` and wrap around past a
bounds check. The full rejection list, each with its own `ElfError`
variant:

| Rejected condition | `ElfError` variant | Why |
|---|---|---|
| Fewer than 64 bytes total | `TooShortForHeader` | Can't even hold a fixed ELF header. |
| Magic isn't `0x7F 'E' 'L' 'F'` | `BadMagic` | Not an ELF file at all. |
| `e_ident[EI_CLASS] != 2` | `NotClass64` | Not 64-bit (KAOS is x86-64 only). |
| `e_ident[EI_DATA] != 1` | `NotLittleEndian` | x86-64 is little-endian; a big-endian file was built for a different CPU. |
| `e_ident[EI_VERSION] != 1` or `e_version != 1` | `BadVersion` | Not the (only) current ELF version. |
| `e_type != ET_EXEC` | `NotExecutable` | Dynamic/relocatable objects unsupported (§1.6). |
| `e_machine != EM_X86_64` | `NotX86_64` | Built for a different CPU architecture. |
| `e_phnum > 32` | `TooManyProgramHeaders` | Hard cap so a corrupt/hostile `e_phnum` can't turn the header walk into an unbounded loop — a real KAOS program has 2 segments; 32 is generous headroom, not a real limit. |
| `e_phentsize != 56` | `BadProgramHeaderSize` | Program header entries must be the fixed ELF64 size for the fixed-offset field reads below to be valid. |
| Program header table doesn't fit in the file | `ProgramHeaderTableOutOfBounds` | `e_phoff + e_phnum * e_phentsize` computed with `checked_add`, then compared against `image.len()`. |
| No `PT_LOAD` entries found | `NoLoadSegments` | Nothing to run. |
| `p_filesz > p_memsz` for some segment | `SegmentFileszExceedsMemsz` | A segment can't have *more* file content than its total memory footprint — §1.3's invariant, violated. |
| `[p_offset, p_offset+p_filesz)` doesn't fit in the file | `SegmentFileRangeOutOfBounds` | The bytes this segment claims to copy don't actually exist in the file. |
| `p_vaddr` not page-aligned, or the page-rounded `[p_vaddr, p_vaddr+p_memsz)` range falls outside `[USER_CODE_BASE, USER_CODE_END)` | `SegmentOutsideCodeWindow` | Enforces the fixed address-space window from §2.3 — a segment cannot claim an address outside the user-code region (e.g. into the stack, heap, or kernel half). |
| Two segments' page-rounded ranges overlap | `SegmentsOverlap` | Two segments claiming the same page would make the "map writable → copy → tighten permissions" sequence (§2.6) apply conflicting final permissions to the same physical page. |
| `e_entry` doesn't fall inside any `PF_X` segment | `EntryNotExecutable` | The very first instruction the CPU would execute must be in mapped, executable memory — otherwise the process would fault (or worse, execute garbage) on its first instruction. |

Two fields from the program header are deliberately **not** validated
against their nominal meaning, with the reason recorded directly as a code
comment:

- **`p_paddr`** is read but ignored — KAOS user programs are mapped by
  virtual address only; physical placement is entirely the PMM's decision,
  and nothing in a paged OS should ever care what "physical address hint" a
  compiled-in linker default happened to produce.
- **`p_align`** is likewise not consulted. Rather than trust the
  linker-supplied alignment hint, the parser validates the *actual*
  alignment it requires directly: every `p_vaddr` must already be a
  multiple of 4 KiB (KAOS only ever creates 4 KiB page mappings — there is
  no huge-page support for user segments).

The overlap check (`SegmentsOverlap`) is a simple pairwise `O(n²)` scan —
deliberately not optimized, because real programs have on the order of 2
segments and the hard cap above bounds it at 32, so even the worst case is
under 500 comparisons of two integers each.

### 2.5 Turning validated segments into page counts

Two small helper methods on `ElfSegment` bridge "the ELF's view" (byte
addresses and sizes) to "the loader's view" (whole 4 KiB pages), matching
the eight-step algorithm's step 3 (§1.7):

```rust
impl ElfSegment {
    pub fn mapped_end(&self) -> u64 {
        page_align_up(self.vaddr + self.memsz)   // page-rounded end of the *memory* footprint
    }
    pub fn page_count(&self) -> usize {
        ((self.mapped_end() - self.vaddr) / PAGE_SIZE_U64) as usize
    }
}
```

Note this is computed from `memsz` (the in-memory footprint, BSS included),
never from `filesz` (the on-disk footprint) — this is the exact fix for the
flat-binary-era bug from §1.4: a page holding only BSS bytes still gets a
`page_count` entry and therefore still gets allocated and mapped.

### 2.6 The loader: `process/loader.rs`

`map_elf_program_image()` implements steps 3–7 of §1.7, once per `PT_LOAD`
segment, inside `try_map_elf_program_image()`:

**Step A — allocate.** `alloc_elf_frames()` walks every segment's
`page_count()` (§2.5) and asks the physical memory manager (PMM) for that
many frames, plus one more frame for the bootstrap stack page (§2.7) — all
under a single PMM lock acquisition, to avoid repeated lock/unlock overhead
in what can be a long allocation loop. On an out-of-memory failure partway
through, every frame already allocated in *this* transaction is released
back to the PMM before returning `ExecError::OutOfMemory` — the caller
never has to deal with a partially-successful allocation.

**Step B — map writable, unconditionally, first.** For every page in every
segment, the loader calls:

```rust
vmm::map_user_code_page(page_va, pfn, /* writable */ true, /* executable */ true);
```

...regardless of what the segment's *final* permissions will be. This is
necessary because step C (copy + zero-fill) needs to write into the page —
even a `R-X` text segment must be writable for a few instructions while its
own bytes are being copied in. This transient over-permissive state is safe
specifically because the entire map→copy→tighten sequence for one program
runs inside `vmm::with_address_space(user_cr3, ...)`, which disables
interrupts for its whole duration: no other code (and definitely no user
code — the process hasn't been scheduled yet) can observe or exploit the
brief window where a would-be-`R-X` page is still writable.

**Step C — copy and zero-fill.** This is where the BSS handling from §1.3–
1.4 is made concrete:

```rust
// Copy exactly the file-backed bytes:
core::ptr::copy_nonoverlapping(image[offset..offset+filesz].as_ptr(), vaddr as *mut u8, filesz);

// Zero-fill everything from the end of the copied bytes through the
// page-rounded segment end, in one pass:
let zero_start = vaddr + filesz;
let zero_len = mapped_end - zero_start;
core::ptr::write_bytes(zero_start as *mut u8, 0, zero_len);
```

The single zero-fill pass deliberately covers *two* different kinds of
"not real file content" bytes at once, because they're contiguous and both
already validated to lie inside this segment's mapped pages:

1. The segment's declared BSS tail (`p_filesz .. p_memsz` — e.g. a `static
   mut COUNTER: u64 = 0` that has no file bytes at all, §1.4), and
2. The leftover space in the last page beyond `p_memsz` up to the
   page-rounded boundary (page granularity means the last page is rarely
   filled exactly) — zeroed so a user program can never observe stale bytes
   left over from whatever this physical frame was previously used for.

**Step D — tighten to final permissions.** Once copy + zero-fill is done,
every page in the segment is re-mapped with its real, final permissions:

```rust
vmm::map_user_code_page(page_va, pfn, seg.writable(), seg.executable());
// where seg.writable()   = (p_flags & PF_W) != 0
//       seg.executable() = (p_flags & PF_X) != 0
```

This is the step that actually delivers W^X (§1.3): a `.text`+`.rodata`
segment (`p_flags = R-X`) ends up genuinely non-writable, and a
`.data`+`.bss` segment (`p_flags = RW-`) ends up genuinely non-executable —
attempting to execute from the data segment, or write to the text segment,
now produces a real CPU protection fault instead of silently succeeding.

**Step E — bootstrap stack.** One page at the top of the user stack window
(`USER_STACK_TOP - PAGE_SIZE`) is mapped writable and zeroed, so the very
first `push`/`call` after entering ring 3 has somewhere to land. This has
nothing to do with the ELF file itself — the stack is not described by any
`PT_LOAD` segment — but it happens in the same transaction because a
process needs both code and a stack before it can run at all. Deeper stack
pages are demand-paged on downward growth by the page-fault handler (§2.7);
only this first page is pre-mapped up front.

**On any failure in steps A–E**, `cleanup_failed_elf_mapping()` performs a
best-effort rollback: it tears down whatever was mapped so far via the
normal VMM address-space-teardown path (which is itself a superset —
see §2.8 — of exactly what this transaction touched), and separately
releases any PMM frames that were allocated but *never* got inserted into a
page table (and are therefore invisible to that teardown path). No
transaction can leak a physical frame regardless of where in steps A–E it
failed.

### 2.7 Fault-time policy: no more demand-paging inside code

Before this loader existed, the *entire* fixed code window was one
undifferentiated region: the page-fault handler in
`memory/vmm/page_fault.rs` treated any non-present fault inside it as "map a
fresh page, read-only" — i.e. it demand-paged code pages into existence
lazily, uniformly read-only, because the old flat-binary loader had no way
to know per-page whether a given byte was meant to be code or writable data
(see §1.4/§2.9 for why that's exactly the problem BSS creates).

Now that every segment is mapped up front with its own final permissions
(§2.6, step D), a page fault *landing inside the code window* can no longer
mean "this page just hasn't been touched yet" — every legitimately-existing
page in that window was already mapped by the loader before the process
ever ran. A fault there now can only mean one of:

- a stale TLB entry,
- a use of a virtual address after its mapping was already torn down, or
- a stack overflow that has spilled past the stack's guard page and landed,
  by pure address-space geometry, inside the code window.

None of these are recoverable by demand-paging a page in — they're genuine
bugs or attacks. `page_fault.rs` therefore now rejects such faults outright
(`PageFaultError::InvalidUserAccess`), the same way a genuine protection
fault would be handled, instead of silently allocating a page:

```rust
if matches!(user_region, Some(UserRegion::Code)) {
    return Err(PageFaultError::InvalidUserAccess { virtual_address, error_code });
}
```

The user stack and user heap windows are unaffected by any of this — they
are not described by ELF segments at all (the ELF file has no opinion about
stack or heap), so they keep their existing demand-paging behavior
(stack: grow on fault, writable+non-executable; heap: same).

### 2.8 Teardown: reusing an existing catch-all instead of exact bookkeeping

When a process exits (or a mid-load failure triggers rollback, §2.6),
`vmm::destroy_user_address_space_with_page_counts()` needs to unmap every
page this process's segments occupied and return the physical frames to the
PMM. A naive design would require the loader to record and hand back an
*exact* list of `(va, page_count)` ranges per segment for teardown to walk.

KAOS's teardown function already had a broader, pre-existing safety net for
a different reason (to guard against any future `mmap`-style user
allocation leaking frames): after unmapping the three known fixed windows
(code/stack/heap) for their *approximate* known extents, it performs a
catch-all scan over the entire user PML4 slot range
(`[USER_CODE_BASE, USER_ADDRESS_SPACE_SCAN_END)`) and reclaims *any* mapping
still present there. Because that catch-all scan already exists and is
already correct and unconditional, the loader doesn't need to build a
separate exact-segment-list teardown path: it hands over the *sum* of all
segments' mapped-page counts as a fast-path hint (so the common case is
still a tight, targeted unmap), and lets the catch-all scan handle anything
that hint doesn't cover exactly — for example, segments that aren't
contiguous with each other. An undercount in the hint cannot leak a frame;
it can only make teardown fall through to the (still correct, just
slightly slower) catch-all path for the pages the hint missed.

### 2.9 The other half: user-program linker scripts

The kernel loader only works if the ELF files it's given actually describe
exactly two clean, non-overlapping, page-aligned `PT_LOAD` segments — R-X
for code/rodata, RW- for data/bss. That shape doesn't happen by accident;
it's produced by an explicit `PHDRS` directive in every user program's
linker script (`main64/user_programs/*/link.ld`):

```ld
PHDRS
{
    text PT_LOAD FLAGS(5); /* R-X : PF_R(4) | PF_X(1) = 5 */
    data PT_LOAD FLAGS(6); /* RW- : PF_R(4) | PF_W(2) = 6 */
}

SECTIONS
{
    . = 0x0000700000000000;   /* USER_CODE_BASE, see §2.3 */

    .text : { *(.text._start) *(.text .text.*) *(.rodata .rodata.*) } :text
    . = ALIGN(4K);            /* force the next PT_LOAD onto a fresh page */
    .data : { *(.data .data.*) } :data
    .bss  : { *(.bss .bss.*) *(COMMON) } :data
}
```

Two details here directly correspond to validation rules from §2.4:

- **The explicit `. = ALIGN(4K);`** between the two section groups exists
  *only* because `SegmentsOverlap` (§2.4) would otherwise reject the file:
  without it, the linker is free to pack the tail of `.rodata` and the
  start of `.data` into the same physical page, which would make the two
  segments' page-rounded ranges overlap.
- **`.bss` is a normal, separate, `SHT_NOBITS` output section again.**
  Before this loader existed, every program's linker script merged `.bss`
  into `.data` as a workaround: the old flat-binary loader only pre-mapped
  `ceil(file_length / PAGE_SIZE)` pages, so any BSS page living past the
  end of the file was silently left unmapped (the motivating bug from
  §1.4). Forcing `.bss` bytes to physically exist in the file (by
  making the output section `SHT_PROGBITS`) was a workaround for a loader
  limitation, not a real requirement of the format. Now that the kernel
  reads `p_memsz` and zero-fills the tail itself (§2.6, step C), `.bss` can
  go back to being what it's supposed to be — implicit, file-space-free,
  zero-initialized storage.

### 2.10 Build pipeline: shipping ELF directly

`main64/build/helper_build_user_programs.sh` used to run
`llvm-objcopy -O binary` on every compiled program to strip it down to a
flat `.bin` blob (raw bytes, no headers at all) before copying it onto the
FAT32 disk image — because the old loader only understood flat binaries.
Since the loader now reads ELF directly, that step is gone: the compiled
ELF executable is copied to the disk image byte-for-byte (still under a
`.bin`-suffixed 8.3 filename for FAT32 compatibility — the loader identifies
the format by its magic bytes, §2.4, not by file extension).

One addition was necessary to keep this affordable: a debug build's
*full* ELF (including symbol table and DWARF debug info) is considerably
larger than the old objcopy'd flat blob was, and can approach the 2 MiB
`USER_CODE_SIZE` window (§2.3) on its own. The build script therefore runs
`llvm-strip --strip-debug` on the copied file before it's shipped. This is
safe precisely because of the sections/segments split from §1.2/1.3:
`--strip-debug` only removes section-header-described content (symbol
tables, `.debug_*` sections) — it does not touch the program header table
or any `PT_LOAD` segment's bytes, so nothing the loader actually reads is
affected.

### 2.11 Tests

Two dedicated test binaries exercise this design end-to-end, in QEMU:

- **`kernel/tests/elf_test.rs`** — unit-level tests against
  `elf::parse_elf64()` directly: constructs synthetic ELF64 images (valid
  and deliberately malformed) and asserts each `ElfError` variant from §2.4
  fires for the condition it's supposed to.
- **`kernel/tests/elf_loader_test.rs`** — integration-level tests against
  `map_program_image_into_user_address_space()`: builds a real two-segment
  image, maps it, and then asserts on the actual resulting page-table state
  — that the text segment is read-only and executable, that the data
  segment is writable and non-executable, that a declared-but-not-in-file
  BSS tail reads back as zero, and that `entry_rip` comes from the image's
  own `e_entry` rather than any fixed constant.

`kernel/tests/process_contract_test.rs` and `kernel/tests/vmm_test.rs` cover
the surrounding contract (image-size validation, `ExecError` mapping,
teardown/rollback frame-accounting, and the code-region fault-rejection
policy from §2.7).
