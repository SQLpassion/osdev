# Migrating the User-Program Loader from Flat Binaries to ELF

This document captures every change required to replace the current flat-binary
loader with a proper ELF loader, plus the latent kernel inconsistencies that
should be cleaned up at the same time. It is intended as a checklist so that
nothing is forgotten when the migration is eventually scheduled.

The document is anchored to file paths and line numbers as of the moment it was
written (against the `main` branch). Use it as a starting map — verify each
location still exists before editing.

---

## 1. Why ELF? The Motivating Bug

Today user programs are built as ELF, then post-processed with
`objcopy -O binary` into flat `.bin` files. The kernel loader simply maps and
copies the raw bytes into the user address space starting at
`USER_CODE_BASE = 0x0000_7000_0000_0000`.

This pipeline silently breaks the moment a program has a non-trivial `.bss`
section:

- `.bss` is `SHT_NOBITS` in the ELF; `objcopy -O binary` strips NOBITS
  sections from the output file.
- The loader computes `code_page_count = ceil(image.len() / 4096)`. Any BSS
  page that lives past the file end is therefore **not** pre-mapped.
- On first write to BSS, the page-fault handler demand-maps that page — but
  for `UserRegion::Code` it tightens permissions to read-only after zero-fill
  (`src/memory/vmm/page_fault.rs` around line 100).
- The next write hits a **protection fault** (`P=1, W=1, U=1`) and the kernel
  panics.

This bit `tui.bin` first, because `lib_tui::screen::SCREEN: static mut [u16; 2000]`
creates a 4000-byte BSS region. It was worked around in the linker scripts by
merging `.bss` into `.data` so the output section becomes `SHT_PROGBITS` and
the BSS bytes are physically written into the `.bin` file. That fix is fragile
and only papers over a deeper design issue: **the loader cannot distinguish
code from data**, so the kernel cannot apply correct page permissions.

An ELF loader fixes the root cause by reading per-segment permissions from the
program headers.

---

## 2. Current Code Map

Anything an ELF loader needs to replace, modify, or interoperate with:

| Concern | File | Notes |
|---|---|---|
| Loader entry points | `main64/kernel_rust/src/process/loader.rs` | `load_program_image`, `map_program_image_into_user_address_space`, `exec_from_fat12` |
| Process descriptor | `main64/kernel_rust/src/process/types.rs` | `LoadedProgram`, `USER_PROGRAM_ENTRY_RIP`, `USER_PROGRAM_INITIAL_RSP`, `image_fits_user_code()` |
| Address-space layout | `main64/kernel_rust/src/memory/vmm/vmm_constants.rs` | `USER_CODE_BASE/SIZE`, `USER_STACK_BASE/TOP`, `USER_HEAP_BASE/END` |
| Region classifier | `main64/kernel_rust/src/memory/vmm/mod.rs` | `classify_user_region()` returns `UserRegion::{Code,Stack,Guard,Heap}` |
| Demand-paging policy | `main64/kernel_rust/src/memory/vmm/page_fault.rs` | derives `writable` and `no_execute` from `UserRegion` — coupled to today's "code-only" assumption |
| Page-table primitives | `main64/kernel_rust/src/memory/vmm/mapping.rs`, `page_table.rs` | `map_user_page(va, pfn, writable)`, `destroy_user_address_space_with_page_counts()` |
| Build pipeline | `main64/build_user_programs.sh` | `cargo build` → `objcopy -O binary` for every user program |
| User-program linker scripts | `main64/user_programs/*/link.ld` | currently merge `.bss/.lbss/COMMON` into `.data` as a workaround |

---

## 3. ELF Loader: Required Changes

### 3.1 Parse ELF program headers

Implement (or pull in) a minimal ELF64 reader. The loader only needs:

- ELF identification (`e_ident`: magic, class=64, data=little-endian, version=1).
- Machine type (`e_machine == EM_X86_64`).
- Type (`e_type == ET_EXEC` for static executables; `ET_DYN` only if relocation
  support is added later).
- Entry point (`e_entry`).
- Program header table (`e_phoff`, `e_phentsize`, `e_phnum`).

For each program header, only `PT_LOAD` segments are interesting. Read:

- `p_vaddr`     — virtual address to map to (must lie inside the user VA window).
- `p_paddr`     — ignore.
- `p_offset`    — offset into the file where segment bytes live.
- `p_filesz`    — number of bytes physically present in the file.
- `p_memsz`     — number of bytes mapped in memory (≥ `p_filesz`; the
                  `[p_filesz, p_memsz)` tail is the BSS portion of the segment).
- `p_flags`     — `PF_R` (0x4), `PF_W` (0x2), `PF_X` (0x1).
- `p_align`     — segment alignment; for 4 KiB paging this should be 0x1000.

Reject and surface errors for:

- Non-ELF input (no magic, wrong class).
- Wrong machine / endianness.
- Any `p_vaddr` outside the user-code window, or any segment that crosses
  region boundaries (code → stack, code → heap).
- `p_filesz > p_memsz`.
- `(p_vaddr % p_align) != (p_offset % p_align)` if you care about virtual /
  file co-alignment (PT_LOAD requirement).
- Overlapping segments.

### 3.2 Map each PT_LOAD segment with its own permissions

Replace the current single contiguous "code window" mapping with a
per-segment map+copy loop. For each `PT_LOAD`:

1. Compute the page-aligned `[seg_start_page, seg_end_page)` range where
   `seg_end_page = page_align_up(p_vaddr + p_memsz)`.
2. Allocate one PFN per page (under one PMM lock scope, like
   `alloc_program_frames()` does today).
3. Map every page **writable** initially via `vmm::map_user_page(va, pfn, true)`
   so the loader can perform the copy/zero work.
4. Copy `p_filesz` bytes from the ELF image at `p_offset` into VA `p_vaddr`.
5. Zero `[p_vaddr + p_filesz, p_vaddr + p_memsz)` (this is the in-segment
   BSS region — explicit, not implicit via `objcopy` stripping).
6. Zero the tail of the last page beyond `p_vaddr + p_memsz` up to
   `seg_end_page` so the user never sees recycled-frame bytes.
7. Tighten permissions to the final policy derived from `p_flags`:
   - `writable = (p_flags & PF_W) != 0`
   - `no_execute = (p_flags & PF_X) == 0`  (requires `EFER.NXE`, already on)
   - User access bit = always true for user segments.
   - Re-`invlpg()` each page after permission tightening.

The flat-binary "everything stays writable" comment in `loader.rs` (around
line 224) becomes obsolete and should be removed: with ELF segment flags we
can finally enforce `.text + .rodata = R-X`, `.data + .bss = RW-`, and the
common security expectation that user code cannot scribble over itself.

### 3.3 Rework the `MapState` rollback structure

Today `MapState` tracks a single contiguous code range plus an optional
stack page. After the rewrite it should hold:

- A list of mapped segments (each with `va`, `page_count`, `writable`).
- The bootstrap stack PFN/state (unchanged in spirit).
- Whether to release owned PFNs or let VMM teardown handle them.

`cleanup_failed_program_mapping()` and `destroy_user_address_space_with_page_counts()`
(see § 5.2) must be updated to iterate per-segment instead of assuming a
single `code_page_count`-long range starting at `USER_CODE_BASE`.

### 3.4 Loader API surface

Public functions that need to keep working but with new internals:

- `load_program_image(file_name) -> Vec<u8>` — unchanged; still reads the
  raw ELF from FAT12.
- `map_program_image_into_user_address_space(image: &[u8]) -> ExecResult<LoadedProgram>`
  — internally switches from flat-copy to ELF parse + per-segment map.
  Returned `LoadedProgram.entry_rip` becomes `header.e_entry` instead of the
  constant `USER_PROGRAM_ENTRY_RIP`.
- `exec_from_fat12(file_name) -> ExecResult<usize>` — unchanged.

`LoadedProgram` itself needs to lose the `code_page_count` field (now a
per-segment list) or generalize it to a list of `(va, page_count)` ranges
that the teardown path can iterate.

### 3.5 Entry-point handling

Today `USER_PROGRAM_ENTRY_RIP = USER_CODE_BASE` and the linker script keeps
`_start` at offset 0. With ELF, the loader must:

- Use `header.e_entry` directly as the initial RIP.
- Validate `e_entry` falls inside an executable segment.
- Drop the linker-script requirement to keep `_start` at image offset 0
  (it can stay for clarity but is no longer enforced by the kernel).

`process/types.rs` should make `USER_PROGRAM_ENTRY_RIP` a fallback for tests
only, not the load-time entry source.

### 3.6 Stack and heap stay as they are

`USER_STACK_TOP - PAGE_SIZE` bootstrap mapping, demand-fault stack growth,
and the user heap region are unaffected. They are user-VA regions that the
ELF binary does not describe, so they keep their dedicated allocation paths.

---

## 4. Page-Fault Handler Cleanup

`src/memory/vmm/page_fault.rs` derives permissions purely from
`classify_user_region()`:

```rust
let writable = !matches!(user_region, Some(UserRegion::Code));
let no_execute = matches!(user_region, Some(UserRegion::Stack));
```

This is the source of the BSS bug described in § 1: it assumes the entire
`USER_CODE` window is read-only executable code. Once the ELF loader installs
correct per-page permissions up front, the handler should:

- **Stop demand-paging inside `UserRegion::Code` at all.** Faults there now
  indicate either a real bug (stale TLB, use-after-unmap) or stack overflow
  spilling into the code region. Treat them as `ProtectionFault` /
  `SegmentationFault`-style termination rather than silently allocating
  read-only zero pages.
- Keep the user-stack growth path (`demand_map_user_stack_growth`) exactly
  as today — that is the only region that legitimately demand-pages.
- Keep the guard-page → protection-fault path.
- Decide explicitly what to do for `UserRegion::Heap` faults (today they
  fall through to `demand_map_leaf_page` with writable=true / NX=false —
  fine, but should be made explicit once Mmap-style heap management lands).

When this cleanup is done, the loader.rs comment
`"Re-protecting pages as read-only would silently break any mutable static"`
becomes obsolete and should be replaced with a comment stating that the new
contract is "each PT_LOAD segment is mapped with exactly its ELF p_flags".

---

## 5. Address-Space Bookkeeping

### 5.1 `classify_user_region()`

Still useful for stack/guard/heap detection. The `UserRegion::Code` variant
no longer means "all read-only code"; it just means "inside the user-program
window". Consider renaming it to `UserRegion::Program` to avoid confusion,
or splitting it into `Text` / `Rodata` / `Data` once segments are tracked
per-process.

### 5.2 `destroy_user_address_space_with_page_counts()`

`src/memory/vmm/mapping.rs` currently tears down by iterating two fixed
contiguous ranges:
`[USER_CODE_BASE, USER_CODE_BASE + code_pages * 4 KiB)` and the stack range.

With ELF this becomes:

- Tear down every mapped page in each `PT_LOAD` segment (loader stores the
  list inside the task's process descriptor).
- Tear down the stack range (unchanged).
- Walk PT/PD/PDP and free empty tables (unchanged).

A simpler interim API is to widen the parameter list to take a slice of
`(va, page_count)` ranges so callers can describe arbitrary segment maps.

### 5.3 Process descriptor

Either:

- Extend the scheduler `TaskEntry` to carry a per-segment list (heap-owned),
  freed in `remove_task` next to `stack` and `fpu_state`; or
- Keep the segment list inside `LoadedProgram`-style metadata attached to
  the task and freed on exit.

Both work; pick whichever keeps `TaskEntry: Copy` semantics intact (current
constraint, see project memory).

---

## 6. Build Pipeline

`main64/build_user_programs.sh` invokes `cargo build` then
`llvm-objcopy -O binary` for every program. Once the kernel reads ELF
directly, the objcopy step is removed entirely:

- The output added to the FAT12 disk image is the ELF file itself (renamed
  to the 8.3 short name like `TUI.BIN` if needed — the extension does not
  matter to the loader, only the magic does).
- Drop the per-program `.bin` filename and use the cargo target output
  directly (or copy it under a `.elf` name for clarity).
- `build_kernel_debug.sh` / `build_kernel_release.sh` references that touch
  the user binaries must be updated to copy the ELF instead of the stripped
  flat blob.

If gradual rollout is wanted, the loader can sniff the file header:

- ELF magic (`0x7F 'E' 'L' 'F'`) → ELF path.
- Anything else → legacy flat path.

This lets you migrate user programs one at a time.

---

## 7. Linker Scripts: Allowed Simplifications

Once the kernel honours ELF segment permissions, the workaround in
`main64/user_programs/*/link.ld` can be undone. The current scripts merge
`.bss/.lbss/COMMON` into `.data` and force a 4 KiB tail align so flat-binary
loading covers BSS pages. With ELF that hack is unnecessary:

- Restore `.bss : { *(.bss .bss.* .lbss .lbss.*) *(COMMON) }` as its own
  output section. It stays `SHT_NOBITS` — the ELF loader honours `p_memsz`
  and zero-fills the in-memory tail itself.
- Keep `.lrodata .lrodata.*` matched into the `.text` section (or its own
  `.rodata` section) — this remains correct under code-model=large.
- Split into PT_LOAD-friendly groupings:
  - `.text + .rodata` → R-X segment
  - `.data + .bss`    → RW- segment
  - Optional `.eh_frame*`, `.got` placement decided per program.
- Drop the `_start at offset 0` invariant comment — the kernel reads
  `e_entry` now.

The linker should naturally produce two `PT_LOAD` program headers (one for
each load group) when using `PHDRS { text PT_LOAD; data PT_LOAD; }` in the
linker script. Verify with `llvm-readelf -l` after each script change.

---

## 8. Validation Checklist (run after each milestone)

1. `llvm-readelf -l user_programs/tui_app/target/.../tui` shows two
   `PT_LOAD` headers with correct `p_flags`.
2. Loading `tui.bin` (now `tui` ELF) does not produce any `[VMM: page fault]`
   debug lines on first frame draw.
3. Writing to a `.text`-resident address from user mode raises a real
   protection fault and terminates the task (proves text is R-X).
4. `static mut SCREEN: [u16; 2000]` works without the linker-script merge
   workaround.
5. A user program with `p_memsz > p_filesz` zero-fills the BSS tail correctly
   (write a test: a `static mut COUNTER: u64 = 0;` and assert it reads 0
   on first access).
6. Process teardown does not leak PFNs across `exec → exit` cycles. Add a
   PMM-free-page-count assertion before and after `exec_from_fat12()`.
7. Demand-faulting an address inside `UserRegion::Code` (e.g. via a wild
   pointer write) terminates the task instead of silently allocating a page.
8. All existing user programs (`hello`, `readline`, `filedemo`, `shell`,
   `tui_app`) still run.

---

## 9. Estimated Touch List

```
main64/kernel_rust/src/process/loader.rs            (rewrite map/copy/teardown)
main64/kernel_rust/src/process/types.rs             (LoadedProgram, entry RIP)
main64/kernel_rust/src/memory/vmm/page_fault.rs     (drop USER_CODE demand-paging)
main64/kernel_rust/src/memory/vmm/mapping.rs        (per-segment teardown API)
main64/kernel_rust/src/memory/vmm/mod.rs            (classify_user_region naming)
main64/kernel_rust/src/scheduler/roundrobin.rs      (TaskEntry segment list, if chosen)
main64/build_user_programs.sh                       (drop objcopy, ship ELF)
main64/build_kernel_debug.sh                        (copy ELF instead of .bin)
main64/build_kernel_release.sh                      (copy ELF instead of .bin)
main64/user_programs/*/link.ld                      (undo .bss-merge workaround,
                                                     add PHDRS PT_LOAD groups)
```

External crate to consider: `goblin` is unsuitable (`std`). A
hand-rolled `ELF64Header` + `ProgramHeader` parser in `no_std` is small
(< 200 lines) and avoids a heavy dependency. Place it under
`main64/kernel_rust/src/process/elf.rs` next to the loader.

---

## 10. Out of Scope (for Reference)

The following items are commonly bundled with ELF support but should be
deferred:

- **Dynamic linking** (`PT_INTERP`, `PT_DYNAMIC`, relocations) — not needed
  for static `ET_EXEC` binaries.
- **PIE / ASLR** — requires relocation handling and a base-address picker.
- **Thread-local storage** (`PT_TLS`) — no userspace threads yet.
- **GNU stack / `PT_GNU_STACK`** — the stack is created by the kernel; the
  segment can be ignored.
- **Symbol tables / debug sections** — kernel does not need them; keep them
  out of the FAT12 image to save space (strip with
  `llvm-strip --strip-debug` before installing).

These can each be added incrementally on top of the static-ET_EXEC loader
without redesigning the foundation.
