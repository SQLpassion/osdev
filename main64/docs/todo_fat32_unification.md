# Implementation Plan: Migrate the Legacy BIOS Boot Path from FAT12 to FAT32

> Goal: Create a **unified storage path** for both boot routes (legacy BIOS + UEFI),
> so that neither the kernel nor the boot chain needs to distinguish between FAT12 and
> FAT32 anymore. This document is intended as a template for a coding AI and describes
> each step concretely.

> ✅ **Implementation status (2026-06-29): IMPLEMENTED & VERIFIED in QEMU.** The legacy
> BIOS path now boots through a FAT32 superfloppy all the way to the Ring-3 shell prompt
> (verified on `-machine pc` + SeaBIOS + IDE, matching the Proxmox VM). Changes made:
> `boot/bootsector.asm` (FAT32 BPB + fixed reserved-sector reads), `boot/functions.asm`
> (FAT12 logic removed), new `kaosldr_64/src/fat32.rs` (no-alloc FAT32 reader) replacing
> `fat12.rs`, `kernel/src/main.rs` (legacy branch mounts FAT32 over ATA at LBA 0), and a
> shared `make_fat32_image.sh` (mtools) called by `build.sh` and `build_kaos_release.sh`
> (the latter keeps `nasm` in Docker but builds the image on the host). `io::fat12` is kept
> for now (read-only decision E3); its removal (§6 / Phase 7) is still optional/pending.

---

## 1. Current State (as of 2026-06-29)

### 1.1 Two boot paths, two filesystems

**Legacy BIOS (FAT12):**

| Stage | File | Task | FS dependency |
|-------|------|------|---------------|
| 1 | `boot/bootsector.asm` + `boot/functions.asm` | 16-bit, loads `KLDR16.BIN`→`0x2000`, `KLDR64.BIN`→`0x3000` | **FAT12** (BPB in the boot sector, `LoadFileIntoMemory` parses root dir @LBA19 + FAT @LBA1, walks the FAT12 chain) — reads via **ATA PIO** (ports `0x1F0–0x1F7`) |
| 2 | `kaosldr_16/*.asm` | 16→64-bit mode switch, A20, E820, video mode, page tables @`0x9000` | **FS-agnostic** |
| 3 | `kaosldr_64/src/{main,fat12,ata}.rs` | loads `KERNEL.BIN`→`0xFFFF_8000_0010_0000` | **FAT12** (`fat12.rs`, hard-coded geometry: root@19, FAT@1, data offset `+33-2`) — reads via **ATA PIO** |
| 4 | `kernel/src/main.rs` (else branch) | mounts FS | **FAT12** via `block::init_ata()` + `io::fat12::init()` |

**UEFI (FAT32):**

- GPT disk, FAT32 EFI System Partition. Kernel: `ahci::init` → `block::init_ahci()`
  → `gpt::find_esp_start_lba()` → `Fat32Volume::mount(esp_lba)` → reads `SHELL.BIN`.

### 1.2 Image creation

- `build.sh` / `build_kaos_release.sh` use **`fat_imgen`** and produce a
  1.44 MB FAT12 superfloppy `kaos64.img` (no partition table, VBR @LBA0).
- The image is written to a SATA SSD via `dd`. The 1.44" floppy geometry is **not**
  used — the image is effectively treated as a flat block device.
- `build_uefi.sh` already uses `sgdisk` + `mformat`/`mcopy` (mtools) for the FAT32 ESP.

### 1.3 Key architectural insight

The kernel already has **a complete, transport-agnostic FAT32 reader**
(`kernel/src/io/fat32.rs`) that reads exclusively through the `block` facade
(`kernel/src/drivers/block.rs`). The facade can be ATA **or** AHCI.
`Fat32Volume::mount(part_lba)` reads the BPB from the first sector of the partition
and derives all geometry values from it — so it works for **any**
partition start LBA (including `0` for a superfloppy).

➜ **The kernel-side change is trivial.** All the real work is in the 16/64-bit
boot chain and the image build.

---

## 2. Target Design

```
Legacy BIOS:  bootsector(FAT32 superfloppy) → KLDR16 → KLDR64(FAT32 reader) → kernel(FAT32 over ATA @LBA0)
UEFI:         firmware → BOOTX64.EFI         →         → kernel(FAT32 over AHCI @ESP-LBA)
                                                            └── identical io::fat32 + io::vfs ──┘
```

- **One** filesystem code path (`io::fat32`) for both boot routes.
- `io::fat12` can be **removed** after a successful migration (see §6).
- The only remaining difference: **transport** (ATA vs. AHCI) and **part_lba**
  (`0` vs. ESP-LBA). That is already cleanly abstracted by the `block` facade.

---

## 3. Key Decisions

These points change the plan substantially. **E3 (read-only) and E5 (image creation via
`mtools`) are decided** (✅). E1/E2/E4 are recommendations with documented alternatives.

### E1 — Partition scheme of the legacy image → **Superfloppy (recommended)**

- **Recommended: FAT32 superfloppy** (FAT32 VBR directly @LBA0, no partition table).
  - `part_lba = 0` in the kernel; no MBR/GPT parsing logic needed.
  - BIOS boots the VBR @LBA0, just like the current FAT12 superfloppy.
  - The `dd`-to-SSD workflow stays unchanged.
  - Minimal diff from today's structure.
- Alternative: MBR + FAT32 primary partition. Requires (a) MBR boot code that chainloads
  the VBR **or** a partition-aware boot sector, and (b) MBR partition parsing in the
  kernel. More code, no functional benefit for this goal. **Not recommended.**

### E2 — How the boot sector finds the loaders → **Reserved sectors (recommended)**

A complete FAT32 directory/chain parser barely fits in the ~420 bytes of code space in a
FAT32 VBR (the FAT32 BPB extends to offset `0x5A`). Two strategies:

- **Recommended: put the loaders in the FAT32 "reserved sectors".**
  - FAT32 reserves a region before the first FAT via the BPB field `reserved sector count`
    (default = 32). We enlarge it (e.g. **64 sectors = 32 KB**) and place
    `KLDR16.BIN` and `KLDR64.BIN` at **fixed LBAs** within that region.
  - The boot sector then does **not** need to parse FAT32 — it just reads fixed sector
    ranges (`LBA 1..N`) via ATA PIO into `0x2000`/`0x3000`. A trivial, robust boot sector.
  - `KERNEL.BIN` and all user programs remain **normal FAT32 files** in the root.
  - Trade-off: the build script must `dd` the loaders to computed offsets in the
    reserved region, and the boot sector must know the (generously rounded) sector counts.
- Alternative A ("full parser in the VBR"): a real FAT32 reader in the 512-byte boot
  sector that searches the root dir and walks the FAT chain sector by sector. Feasible
  (real FAT32 VBRs do this), but very tight and error-prone in 16-bit real mode. **Only if
  reserved sectors are undesirable.**
- Alternative B ("everything reserved"): place `KERNEL.BIN` in the reserved region too →
  then **`kaosldr_64` would not need** a FAT32 reader either. Minimal code, but the kernel
  is no longer a visible file and the reserved region must be ~1 MB. Less clean.

#### E2 in depth — why these steps are necessary

This is conceptually the trickiest part of the plan, so here is the full rationale: first
the **core problem**, then **why FAT32 makes it worse**, then **why exactly these steps**
are the solution.

##### The core problem: a chicken-and-egg dilemma

At boot there is one hard fact: **the BIOS loads only the very first sector (LBA 0)** of the
disk to `0x7C00` and jumps into it. The BIOS does nothing more. From there on, everything is
our responsibility.

That creates a bootstrapping problem:

| Component | has a FAT32 driver? | already loaded at boot? |
|---|---|---|
| Boot sector (512 B @LBA0) | no | yes (by the BIOS) |
| `kaosldr_64` (Rust) | yes (we build it in step C) | **no** — must be loaded first |
| Kernel | yes (`io::fat32`) | **no** — must be loaded first |

➜ The boot sector is the **only** component guaranteed to run — but it has **no** filesystem
driver. Yet it must fetch `KLDR16.BIN` and `KLDR64.BIN` from disk. **Something has to start
the chain without being able to read a filesystem.**

##### Variant 1: make the boot sector FAT32-aware (what we avoid)

For the boot sector to load a file *"by name"* from FAT32, it would have to be a mini FAT32
driver. Concretely it would need to:

1. **Read the BPB** and compute geometry (`fat_start_lba`, `data_start_lba`, …).
2. **Search the root directory** to find the file's directory entry — on FAT32 the root dir
   is **no longer a fixed region** (unlike FAT12!), but a **cluster chain** in the data
   region. So you must do cluster→LBA math **and** follow the chain, skipping LFN (long file
   name) entries and matching 8.3 names.
3. **Follow the file's cluster chain through the FAT** and read each data cluster into memory.

**Why this is especially hard on FAT32:** today's FAT12 boot sector (`boot/functions.asm`,
`LoadFileIntoMemory`) loads the **entire** root directory (14 sectors @LBA19) and the
**entire** FAT (18 sectors @LBA1) into low memory and walks them there. That **only works
because FAT12 is tiny**: a fixed root dir at a fixed LBA (19), and the whole FAT fits in ~9
sectors. On FAT32 this breaks:

- **No fixed root dir** — it lives as a cluster chain somewhere in the data region.
- **The FAT is huge.** For a 64 MB volume the FAT is hundreds of KB to MBs. But in 16-bit
  real mode there are only ~640 KB of conventional memory and only tiny buffers — you
  **cannot load the whole FAT**. You would have to load FAT sectors **on demand** (read a
  FAT sector → find the next cluster → maybe read another FAT sector → …), which is
  significantly more code.

And all of this must fit in the **~420 bytes of code space** a FAT32 VBR offers at all
(512 B − 90 B FAT32 BPB − 2 B signature), for **two** files, in 16-bit assembly. Feasible
for experts, but tight and error-prone.

##### Variant 2: reserved sectors (our decision)

The idea: **if the boot sector always knows exactly which LBA the loaders live at, it needs
no filesystem driver at all.** It then simply does "read N sectors starting at LBA X" — which
it can do in a handful of instructions with the existing `ReadSector` routine.

For that we need a place on the disk that **(a) has a fixed, predictable address** and
**(b) is never touched by the filesystem**. Those are exactly the FAT32 **reserved sectors**.

Layout recap:

```
LBA 0      : VBR (boot sector)           ┐
LBA 1      : FSInfo (from mformat)       │
LBA 6      : backup boot sector          │  reserved region (before the FAT)
LBA 8      : KLDR16.BIN  ← fixed slot    │
LBA 16     : KLDR64.BIN  ← fixed slot    ┘
LBA ...    : FAT #1, FAT #2
LBA ...    : data region: KERNEL.BIN, *.BIN, … (normal FAT32 files)
```

Why each step is necessary:

1. **Enlarge the reserved-sector count via the BPB (`mformat -R 64`).** The reserved region
   is the **only** place on a FAT32 volume where you can stash raw data the filesystem is
   guaranteed to leave alone (it sits **before** FAT #1; FAT and data come after). The
   default is 32 sectors — of which LBA 1 (FSInfo) and LBA 6 (backup boot) are taken, and it
   is not clearly "ours". With 64 sectors we carve out a **guaranteed, FS-safe area at known
   LBAs**.
2. **Place KLDR16/KLDR64 at fixed LBAs (≥ LBA 8).** *Fixed* means the boot sector knows the
   address as a constant (`EQU`) and has to parse **nothing**. "≥ 8" ensures we do not
   overwrite FSInfo (LBA 1) or the backup boot sector (LBA 6).
3. **The boot sector reads fixed sector ranges via ATA PIO.** This is the actual payoff:
   instead of a FAT32 parser, just "read `KLDR64_MAX_SECTORS` from `KLDR64_LBA` into
   `0x3000`" — with the unchanged `ReadSector` routine. The hard part (FAT32 in 512 bytes)
   **disappears entirely**.
4. **KERNEL.BIN + user programs stay normal FAT32 files.** They do **not** need to be in the
   reserved region, because they are read **later** — by `kaosldr_64` (Rust, no 512-byte
   limit) and by the kernel (`io::fat32`). Once `kaosldr_64` runs, the chain *has* a full
   FAT32 driver. Only the tiny early loaders need the reserved region, so it stays small
   (32 KB) and the kernel remains a visible, normal file.
5. **Trade-off (build script + fixed sector counts).** Because the build script must `dd` the
   loaders to the fixed offsets and file sizes vary per build, the boot sector uses
   **generously rounded fixed max sector counts** (e.g. KLDR16 ≤ 8, KLDR64 ≤ 40). It may read
   a few sectors too many (harmless — the loader is still fully in memory). The `check_fits`
   guard in the build hard-fails if a loader ever grows beyond its slot.

**In one sentence:** instead of teaching the 512-byte boot sector to read a complex FAT32
filesystem (Variant 1, tight & fragile), we place the two early loaders at **fixed,
FS-protected addresses** in the reserved region (Variant 2) — turning the boot sector into a
trivial "read fixed sectors", while the *real* FAT32 parsing happens only in `kaosldr_64` and
the kernel, where there is room and a full Rust implementation.

### E3 — Write access → ✅ **DECIDED: read-only accepted for the legacy path**

⚠️ **Deliberately accepted functional regression:**

- `io::fat12` supports **writing/deleting** (`write`, `delete`, `seek` with write mode),
  used e.g. by `filedemo` and possibly shell commands.
- `io::fat32` is currently **read-only**: `open(Write)`, `write`, `delete` return
  `FsError::Unsupported` (see `Fat32Fs` in `fat32.rs`).
- Since the **UEFI path is already read-only today**, the migration merely brings the
  legacy path to the same state — no new special case.

**Decision:** The migration is performed **read-only**; both boot paths then behave
identically (read-only). The loss of write/delete access on the legacy path is
deliberately accepted. FAT32 write/delete is **not** part of this migration and can be
added later as a separate, optional work package (see §7, Phase 6 — marked
"optional/future").

### E4 — Transport of the legacy path → **Keep ATA PIO**

The legacy path reads via ATA PIO today (ports `0x1F0–0x1F7`) — in the boot sector, in
`kaosldr_64`, and in the kernel (`block::init_ata()`). This stays **unchanged**; only the
filesystem on top of it changes. (AHCI remains reserved for the UEFI path.)

### E5 — Image creation → ✅ **DECIDED: `mtools` (`mformat`/`mcopy`) instead of `fat_imgen`**

`fat_imgen` is a pure FAT12/FAT16 tool and **cannot do FAT32** — it is removed entirely.
Both build scripts will use the same toolchain as `build_uefi.sh`: **`mtools`**
(`mformat` + `mcopy`). Prerequisite: `mtools` in the dev container (already present) and
locally (macOS: `brew install mtools`).

**Decided procedure** (legacy FAT32 superfloppy):

1. `dd` creates an empty backing file (≥ 64 MB, so FAT32 does not degrade to FAT16/12).
2. `mformat -F -R 64` creates the FAT32 filesystem **including a correct BPB** and a
   large reserved region (64 sectors) for the loaders.
3. `mcopy` copies `KERNEL.BIN`, `SHELL.BIN`, and all other programs/files as
   normal FAT32 root entries (8.3 uppercase).
4. `dd` writes `KLDR16.BIN`/`KLDR64.BIN` to fixed LBAs in the reserved region (E2).
5. **BPB-preserving boot-sector overlay** (see below; resolves R1).

**Decided boot-sector overlay procedure** (resolves risk R1 concretely):

`mformat` writes its own FAT32 BPB **and** its own boot code to LBA 0. We want our boot
code, but must preserve the **BPB (pure data fields, no code)** that `mformat` produced
exactly, because the kernel / `kaosldr_64` read it. Deterministic approach:

```sh
# a) Save the FAT32 BPB fields (offset 0x0B..0x5A = 79 bytes) from the mformat image.
dd if=kaos64.img of=bpb_save.bin bs=1 skip=11 count=79 2>/dev/null
# b) Write our complete boot sector (JMP@0x00, OEM, code from 0x5A, signature 0x55AA) to LBA0.
dd if=boot/bootsector.bin of=kaos64.img bs=512 count=1 conv=notrunc 2>/dev/null
# c) Write the saved BPB fields back → BPB = mformat, code = ours.
dd if=bpb_save.bin of=kaos64.img bs=1 seek=11 count=79 conv=notrunc 2>/dev/null
```

Consequence for `bootsector.asm`: the FAT32 BPB defined there is only a **placeholder for
correct code offsets** (it is overwritten with the `mformat` values in step c). Only these
matter: a valid `JMP`@`0x00` to the code at `0x5A`, the code region itself, and the
signature `0x55AA`@`0x1FE`.

---

## 4. Affected Components — Overview

| # | File | Change | Effort |
|---|------|--------|--------|
| A | `boot/bootsector.asm` | FAT12 BPB → FAT32 BPB; remove FAT12 parsing; read fixed reserved LBAs | medium |
| B | `boot/functions.asm` | remove `LoadFileIntoMemory` (FAT12 chain walk); keep only `ReadSector` | small |
| C | `kaosldr_64/src/fat32.rs` (**new**) | no-alloc FAT32 reader: load `KERNEL.BIN` via ATA | medium-large |
| D | `kaosldr_64/src/main.rs` | `fat12::load_kernel_into_memory` → `fat32::…` | small |
| E | `kaosldr_64/src/fat12.rs` | remove (after C) | small |
| F | `kernel/src/main.rs` | else branch: FAT12 mount → FAT32 mount (`Fat32Volume::mount(0)` over ATA) | small |
| G | `build.sh` | `fat_imgen` → `mformat`/`mcopy` + reserved region + boot-sector overlay | medium |
| H | `build_kaos_release.sh` | same (Docker block) | medium |
| I | `kernel/src/io/fat12/**`, `io/mod.rs` | remove after migration (optional, §6) | small |
| J | `docs/boot_bios.md` | document FAT12→FAT32 | small |

---

## 5. Detailed Implementation Plan (reserved-sectors variant, E2 recommended)

### Fixed disk layout (legacy FAT32 superfloppy)

```
LBA 0            : FAT32 VBR (boot sector, our code + mformat BPB)
LBA 1            : FS Information Sector (FSInfo, from mformat)        [part of reserved]
LBA 6            : Backup boot sector (from mformat)                   [part of reserved]
LBA RSV_KLDR16.. : KLDR16.BIN   (e.g. LBA 8,  max 8 sectors  = 4 KB)
LBA RSV_KLDR64.. : KLDR64.BIN   (e.g. LBA 16, max 32 sectors = 16 KB)
... (total reserved sector count e.g. 64)
LBA RSV_END..    : FAT #1, FAT #2
... data region  : root-dir cluster, KERNEL.BIN, *.BIN, *.BAS, *.TXT (normal FAT32 files)
```

Define constants centrally (as `EQU` in the boot sector, as shell variables in the build script):

```
RESERVED_SECTORS  = 64
KLDR16_LBA        = 8     ; KLDR16_MAX_SECTORS = 8
KLDR64_LBA        = 16    ; KLDR64_MAX_SECTORS = 32
```

> The build script must **hard-fail** (guard) if the max sizes are exceeded, so an
> oversized loader does not silently spill into the FAT.

> ⚠️ **Critical load-address constraint (learned during implementation):** the loaders are
> read into low memory *while this boot sector executes at 0x7C00*. Since KLDR64 is read to
> `0x3000`, its sector count MUST satisfy `0x3000 + KLDR64_MAX_SECTORS*512 <= 0x7C00`
> (i.e. `<= 38`), otherwise the multi-sector read overwrites the running boot sector and the
> CPU executes the over-written bytes → endless #UD. We use **32** (ends at `0x7000`, ~3 KB
> margin). The old FAT12 loader never hit this because it read the *exact* file length
> (~10 sectors), not a padded maximum. Likewise KLDR16 at `0x2000` must stay below KLDR64 at
> `0x3000` (`<= 8` sectors). Read KLDR64 first, then KLDR16 (it is executed last).

---

### Step A+B — boot sector + functions.asm (FAT32 superfloppy, reserved read)

**`boot/bootsector.asm`:**

1. **Replace the BPB** with a complete **FAT32 BPB** (offsets 0x0B–0x59):
   - `BytesPerSector=512`, `SectorsPerCluster=<from mformat>`, `ReservedSectors=64`,
     `NumFATs=2`, `RootEntries=0`, `TotalSectors16=0`, `Media=0xF8`, `SectorsPerFAT16=0`,
     `TotalSectors32=<large>`, `SectorsPerFAT32=<from mformat>`, `RootCluster=2`,
     `FSInfoSector=1`, `BackupBoot=6`, extended signature `0x29`, `FileSystem="FAT32   "`.
   - ⚠️ These values **must exactly** match the filesystem produced by `mformat`.
     **Recommended implementation:** in the build (step G), **preserve** the BPB written
     by `mformat` and overwrite only the **code region** (`0x5A..0x1FE`) with our boot code
     (leaving BPB bytes `0x00..0x5A` + signature untouched). Then `bootsector.asm` itself
     need not contain an exact BPB — the `DB` placeholders only serve to keep the code
     offset correct.
2. **Simplify `Main:`** — remove the entire FAT12 load/search construct. New flow:
   ```asm
   ; Read KLDR64.BIN from reserved sectors → 0x3000
   MOV  BX, KLDR64_MAX_SECTORS
   MOV  ECX, KLDR64_LBA
   MOV  EDI, KAOSLDR64_OFFSET      ; 0x3000
   CALL ReadSector
   ; Read KLDR16.BIN from reserved sectors → 0x2000
   MOV  BX, KLDR16_MAX_SECTORS
   MOV  ECX, KLDR16_LBA
   MOV  EDI, KAOSLDR16_OFFSET      ; 0x2000
   CALL ReadSector
   ; Execute Stage 2
   CALL KAOSLDR16_OFFSET
   ```
3. `ReadSector` (ATA PIO) stays — still used from `functions.asm`.
4. Keep the ordering: load KLDR16 **last** into `0x2000`, then execute it (matches today's
   behavior — KLDR16 calls KLDR64 @`0x3000`).

**`boot/functions.asm`:**

- **Remove** `LoadFileIntoMemory`, `.FindFileInRootDirectory`, the FAT12 chain logic, the
  `Failure` path, and the FAT12-specific variables (`Cluster`, `FileName`, …).
- **Keep** `PrintLine`, `ReadSector` (and possibly `Check_ATA_BSY/DRQ`).

**Size check:** without the FAT12 parser the code is much smaller — it easily fits next to
the FAT32 BPB within 512 bytes.

---

### Step C — `kaosldr_64/src/fat32.rs` (no-alloc FAT32 reader)

A new module analogous to the existing `fat12.rs`, but with FAT32 geometry and **on-demand
FAT-sector reads** (the FAT32 FAT is too large to read entirely). Uses the existing
`ata::read_sectors`. **No** heap allocation.

Structure (modeled on `kernel/src/io/fat32.rs`, but without `Vec`/the `block` facade):

```rust
// Reads the BPB from LBA 0 (superfloppy ⇒ part_lba = 0).
struct Fat32Geometry {
    sec_per_clus: u32,
    fat_start_lba: u32,
    data_start_lba: u32,   // = fat_start + num_fats * fat_sz_32
    root_cluster: u32,
}

// Use a single sector-sized static/stack buffer strategy (no alloc):
//  - 512-byte stack buffer for BPB / directory sector / FAT sector.
//  - Stream the file directly to KERNEL_BUFFER (0xFFFF_8000_0010_0000).

fn mount() -> Fat32Geometry { /* parse BPB @LBA0 (fields: see kernel/io/fat32.rs §mount) */ }

fn find_in_root(geo, name_8_3) -> Option<(first_cluster, file_size)> {
    // Walk the root cluster chain, read sec_per_clus sectors per cluster,
    // check 16 dir entries/sector; handle LFN (attr==0x0F) and 0xE5/0x00.
}

fn next_cluster(geo, cluster) -> u32 {
    // FAT sector on demand: fat_sector = fat_start + (cluster*4)/512;
    // offset = (cluster*4) % 512; value & 0x0FFF_FFFF; >=0x0FFF_FFF8 ⇒ EOC.
}

pub unsafe fn load_kernel_into_memory(name: &[u8;11]) -> Result<i32, &'static str> {
    let geo = mount();
    let (first, _size) = find_in_root(&geo, name).ok_or("Kernel file not found")?;
    // Walk the cluster chain, read each cluster (sec_per_clus sectors) into
    // a running KERNEL_BUFFER; return the sector count.
}
```

Notes:
- Follow the project's code comment style and `SAFETY:` blocks.
- `cluster_to_lba(c) = data_start_lba + (c-2) * sec_per_clus`.
- Add a sufficient cluster-loop guard against corrupted chains (like the kernel reader).
- Keep the return value in **sectors** (like `fat12.rs`), because `main.rs` computes
  `kernel_size = sectors * 512` from it.

> Optional: instead of `find_in_root` it would in principle suffice to load only
> `KERNEL.BIN` — but keep the interface generic (`name: &[u8;11]`) as it is today.

---

### Step D+E — `kaosldr_64/src/main.rs` and removing `fat12.rs`

- `main.rs`: `mod fat12;` → `mod fat32;`, `use fat12::load_kernel_into_memory;` →
  `use fat32::load_kernel_into_memory;`. The `KERNEL  BIN` call stays identical.
- Delete `kaosldr_64/src/fat12.rs`.
- Check whether `ata.rs::write_sectors` (currently `#[allow(dead_code)]`) is still needed —
  otherwise leave it or remove it.

---

### Step F — `kernel/src/main.rs` (switch else branch to FAT32)

Current (legacy branch):

```rust
drivers::ata::init();
drivers::block::init_ata();
io::fat12::init();
io::vfs::mount(alloc::boxed::Box::new(io::fat12::Fat12Fs));
io::vfs::read_file("shell.bin")...
```

New:

```rust
drivers::ata::init();
drivers::block::init_ata();
// Superfloppy: the FAT32 VBR is at LBA 0.
let vol = io::fat32::Fat32Volume::mount(0).expect("FAT32 mount (ATA, LBA0) failed");
io::vfs::mount(alloc::boxed::Box::new(io::fat32::Fat32Fs::new(vol)));
io::vfs::read_file("shell.bin")...
```

**Optional unification** (recommended, reduces branching): both branches mount the same FS
type; they differ only in `(transport_init, part_lba)`:

```rust
let (part_lba) = if uefi {
    drivers::ahci::init(); drivers::block::init_ahci();
    io::gpt::find_esp_start_lba().expect("ESP not found")
} else {
    drivers::ata::init(); drivers::block::init_ata();
    0
};
let vol = io::fat32::Fat32Volume::mount(part_lba).expect("FAT32 mount failed");
io::vfs::mount(alloc::boxed::Box::new(io::fat32::Fat32Fs::new(vol)));
let shell_image = io::vfs::read_file("shell.bin").expect("failed to load SHELL.BIN");
```

> The `uefi` detection (`booted_via_framebuffer && !primary_present()`) stays unchanged.
> ⚠️ Make sure the `Fat32Volume::read_file` size limit (`TooLarge`) is large enough for
> the largest file read through the kernel (the kernel loads `SHELL.BIN` etc.; `KERNEL.BIN`
> is loaded by the loader, not the kernel). Raise the limit if needed.

---

### Step G+H — Migrate the build scripts to FAT32

Replace `fat_imgen` (FAT12) with `mtools`. **Recommended approach** (analogous to
`build_uefi.sh`, but superfloppy instead of GPT):

```sh
IMG=kaos64.img
RESERVED_SECTORS=64
KLDR16_LBA=8
KLDR64_LBA=16

rm -f "$IMG"
# 1) Create the backing file (generous size, e.g. 64 MB)
dd if=/dev/zero of="$IMG" bs=1048576 count=64 2>/dev/null

# 2) Format a FAT32 superfloppy, increase the reserved sectors (mtools: -R)
#    '-F' forces FAT32. Device string "::" + image via -i.
mformat -i "$IMG" -F -R "$RESERVED_SECTORS" ::

# 3) Copy normal files as FAT32 root entries (8.3 uppercase)
mcopy -i "$IMG" target/x86_64-unknown-none/debug/kernel.bin ::/KERNEL.BIN
mcopy -i "$IMG" user_programs/hello/hello.bin               ::/HELLO.BIN
mcopy -i "$IMG" user_programs/readline/readline.bin         ::/READLINE.BIN
mcopy -i "$IMG" user_programs/filedemo/filedemo.bin         ::/FILEDEMO.BIN
mcopy -i "$IMG" user_programs/shell/shell.bin               ::/SHELL.BIN
mcopy -i "$IMG" user_programs/tui_app/tui.bin               ::/TUI.BIN
mcopy -i "$IMG" user_programs/kbasic/kbasic.bin             ::/KBASIC.BIN
mcopy -i "$IMG" SFile.txt ::/SFILE.TXT
mcopy -i "$IMG" BigFile.txt ::/BIGFILE.TXT
mcopy -i "$IMG" user_programs/kbasic/src/demo.bas ::/DEMO.BAS

# 4) Size guards: loaders must not exceed their reserved slot
check_fits() {  # $1=file $2=max_sectors
  local sz=$(wc -c < "$1"); local secs=$(( (sz + 511) / 512 ))
  [ "$secs" -le "$2" ] || { echo "ERROR: $1 = $secs sectors > max $2"; exit 1; }
}
check_fits kaosldr_16/kldr16.bin 8
check_fits target/x86_64-unknown-none/debug/kldr64.bin 32

# 5) Write the loaders into the reserved region (conv=notrunc, seek in sectors)
dd if=kaosldr_16/kldr16.bin                         of="$IMG" bs=512 seek="$KLDR16_LBA" conv=notrunc 2>/dev/null
dd if=target/x86_64-unknown-none/debug/kldr64.bin   of="$IMG" bs=512 seek="$KLDR64_LBA" conv=notrunc 2>/dev/null

# 6) BPB-preserving boot-sector overlay (decided procedure, see E5):
dd if="$IMG" of=bpb_save.bin bs=1 skip=11 count=79 2>/dev/null            # save mformat BPB
dd if=boot/bootsector.bin of="$IMG" bs=512 count=1 conv=notrunc 2>/dev/null  # write our boot sector
dd if=bpb_save.bin of="$IMG" bs=1 seek=11 count=79 conv=notrunc 2>/dev/null  # restore BPB fields
rm -f bpb_save.bin
```

Adjustments:
- Migrate **both** `build.sh` (debug, local) and `build_kaos_release.sh` (Docker block).
- Ensure `mtools` is available in the dev container **and** locally (macOS:
  `brew install mtools`) — it is already a prerequisite for `build_uefi.sh`.
- `qemu-img convert` to `kaos64.qcow2` (release script) stays unchanged.
- `fat_imgen` is **removed entirely** from both scripts.

> **Boot-sector overlay (step 6) — decided (E5):** first `mformat`, then save the BPB field
> region `0x0B..0x5A` (79 bytes) from the image, write the **complete** `bootsector.bin` to
> LBA 0, and write the saved BPB fields back. Result: BPB = from `mformat` (correct
> geometry), code + `JMP`@0x00 + signature = ours. For this, `bootsector.bin` must contain a
> valid `JMP`@0x00 to the code at `0x5A` and the signature `0x55AA`@`0x1FE`; its own BPB is
> just an offset placeholder.

---

## 6. Cleanup After a Successful Migration (optional, depends on E3)

Once FAT32 is verified working **and** the write decision (E3) has been made:

- If **read-only** is accepted: remove `kernel/src/io/fat12/**` entirely, drop `io::fat12`
  from `io/mod.rs`, remove the `Fat12Fs` references.
- Remove `kaosldr_64/src/fat12.rs` (already in step E).
- Mark `docs/fat12.md` as "historical/removed" or delete it.
- ⚠️ If write access is still needed, **remove FAT12 only after FAT32 write (Phase 6)
  exists.**

---

## 7. Recommended Phasing / Ordering

| Phase | Content | Verification |
|-------|---------|--------------|
| 1 | Migrate the **build script** to a FAT32 superfloppy (step G), leave the boot sector unchanged for now, test image creation only | `mdir -i kaos64.img ::` shows all files; `file kaos64.img` / hexdump shows the FAT32 BPB |
| 2 | **kaosldr_64 FAT32 reader** (steps C/D/E) | Boot in QEMU: the kernel is loaded (kernel banner appears) |
| 3 | Migrate the **boot sector** to reserved read (steps A/B) + build steps 5/6 | QEMU boots up to the kernel; KLDR16/64 loaded correctly |
| 4 | Migrate the **kernel mount** to FAT32 (step F) | QEMU: shell starts, `ls`/file reads work |
| 5 | **Unification** of main.rs + test on real HW / SSD via `dd` | Boot from SSD; UEFI path still green |
| 6 | (Optional, E3) implement **FAT32 write/delete** | `filedemo` writes/deletes successfully |
| 7 | (Optional) **remove FAT12** (§6) | Build green, all tests green |

> Each phase is testable on its own in QEMU (legacy: `kaos64-bios-vm.sh` / the `build.sh`
> hint; the UEFI path must **stay** green after each phase, since it uses the same
> `io::fat32`).

---

## 8. Risks & Pitfalls

- **R1 — BPB consistency boot sector ↔ mformat.** ✅ Resolved by the decided overlay
  procedure (E5 / §5 step G/6): the BPB written by `mformat` is preserved; only code +
  `JMP` + signature are ours. Remaining diligence: after the build, verify via hexdump that
  `0x0B..0x5A` contains the `mformat` values, that `0x52..0x5A` contains the string
  `"FAT32   "`, and that the `JMP`@0x00 points to the code.
- **R2 — `SectorsPerCluster` & `ReservedSectors`.** The fixed loader LBAs must lie within
  `ReservedSectors` and must **not** overwrite FSInfo (LBA 1) / the backup boot sector
  (LBA 6). Choose loader slots at LBA ≥ 8. `mformat -R 64` provides enough room.
- **R3 — FAT32 minimum size.** FAT32 requires a minimum cluster count (≥ 65525 data
  clusters). With too small an image, `mformat` may "degrade" to FAT16/FAT12.
  ⇒ Choose a large enough image (e.g. 64 MB) **and** verify via hexdump after `mformat`
  that offset `0x52..0x5A` contains `"FAT32   "`.
- **R4 — Loader larger than its reserved slot.** Release builds can be larger. The
  `check_fits` guards (step G/4) prevent silent data loss. Choose slots generously.
- **R5 — ATA PIO on real legacy HW.** The boot sector reads via ATA PIO ports. If the SATA
  SSD runs in UEFI/AHCI-only mode, the legacy boot fails (unchanged from today). Real HW is
  intended for the UEFI path anyway (see project memory).
- **R6 — `read_file` size limit (`TooLarge`) in the kernel.** Before the migration, verify
  the limit covers the largest file the kernel reads.
- **R7 — 8.3 names / LFN.** `mcopy` creates LFN entries for lowercase/long names. The FAT32
  reader skips LFN (attr `0x0F`) and matches the 8.3 entry — keep filenames deliberately
  8.3-compliant (uppercase in the `mcopy` target: `::/KERNEL.BIN`).
- **R8 — KLDR16/KLDR64 ordering.** Today KLDR16 is loaded last into `0x2000` and executed;
  KLDR16 jumps to KLDR64 @`0x3000`. Keep this ordering in the new boot sector.

---

## 9. Verification Checklist

- [ ] `mdir -i kaos64.img ::` lists `KERNEL.BIN`, `SHELL.BIN`, all `*.BIN/*.BAS/*.TXT`.
- [ ] Hexdump LBA0 offset `0x52`: `"FAT32   "`; offset `0x1FE`: `55 AA`.
- [ ] Hexdump LBA `KLDR16_LBA`/`KLDR64_LBA`: contains loader code (not 0x00).
- [ ] QEMU legacy boot (`build.sh` + `kaos64-bios-vm.sh`): up to the user shell.
- [ ] Shell: read a file (e.g. `cat SFILE.TXT` / `type`), start a program (`hello`).
- [ ] UEFI boot (`build_uefi.sh`) **still** green.
- [ ] `cargo test -p kernel` green (`io::fat32`/`vfs` tests; FAT12 tests removed if applicable).
- [ ] Boot from a real SSD via `dd` (if legacy HW is available).

---

## 10. Rollback

- The migration is phased and git-revertible. The critical cut is Phase 4 (kernel mount).
  Until then, FAT12 remains available in the kernel.
- **Remove `io::fat12` only in Phase 7** — until then, a rollback to FAT12 is possible by
  reverting `main.rs` (step F) + the old build scripts.
