# AHCI bring-up: loading `SHELL.BIN` from the ESP on the UEFI boot path

> **Status:** implementation spec (not yet implemented). Branch: `feature/ahci-driver`.
> **Audience:** an AI coding agent that will implement this end-to-end in a fresh session.
> **Language:** the kernel/code is English; design discussion with the maintainer is in German.

## 1. Goal & scope

Today the UEFI boot path is a dead end: the kernel boots, prints a framebuffer
message, and halts (`kernel/src/main.rs`, the `if booted_via_framebuffer(...) && !primary_present()`
block, ~line 215, ends in `idle_loop()`). It cannot start the user-space shell
because:

1. A UEFI machine has **no legacy IDE ports**, so the existing ATA-PIO driver
   (`drivers::ata`) cannot reach the disk. The disk is a SATA device behind an
   **AHCI** controller.
2. The files (`SHELL.BIN`, …) live on the **FAT32** EFI System Partition (ESP),
   but the kernel only has a **FAT12** filesystem driver.

This task implements a **deliberately minimal, self-contained vertical slice** that
makes the UEFI path load and run `SHELL.BIN` from the ESP via AHCI:

- AHCI sector **reads** (the experimental driver already exists; we verify + use it).
- **GPT** parsing to locate the ESP.
- A **minimal read-only FAT32** reader (find one file in the root directory, follow
  its cluster chain, return the bytes).
- Wiring it into the boot flow and handing the bytes to the existing user-space
  loader.

### Explicitly OUT of scope (do NOT do these here)

- **No** `BlockDevice`/VFS trait abstraction. Call `drivers::ahci::read_sectors`
  directly. (That generalization is a separate, later task.)
- **No** AHCI **write** support, no multi-PRDT / multi-sector-per-command
  optimization. Read-only is enough.
- **No** generalization of the existing FAT12 driver. Write a **new, separate**
  FAT32 module. Do not touch `io::fat12`.
- **No** changes to the legacy BIOS boot path, the bootsector, `kaosldr_16`, or
  `kaosldr_64`.
- **No** long filename (LFN) support, no subdirectory traversal, no FAT16/FAT12
  detection in the new module. The ESP root directory and 8.3 names are enough.

Hardcoding is acceptable where noted (e.g. falling back to a fixed ESP LBA), as
long as it is marked with a `TODO` and the primary path reads the real value.

### Build on the existing AHCI driver — do NOT rewrite it

This task **builds on top of the existing `kernel/src/drivers/ahci.rs`**. Do not
create a second/parallel AHCI implementation and do not rewrite that file. Reuse
its public API (`ahci::init`, `ahci::read_sectors`, `ahci::AhciError`) as-is. You
may make **small, additive** changes to `ahci.rs` *only if a step strictly
requires it* (e.g. exposing an extra helper), and only after confirming the
existing read path works (Step 1). Larger reworks (write support, multi-PRDT,
interrupt-driven completion, the `BlockDevice` trait) are explicitly deferred to
later tasks — see §1 out-of-scope.

## 2. Background: relevant existing code

### 2.1 The AHCI driver (already present, "experimental")

`kernel/src/drivers/ahci.rs` already implements controller discovery and
single-port single-slot reads. Public surface:

```rust
pub fn init();                                   // finds AHCI controller via PCI,
                                                 // maps ABAR, rebases first SATA port
pub fn read_sectors(buffer: &mut [u8], lba: u32, // reads `sector_count` 512-byte
                    sector_count: u8)            // sectors starting at `lba`
    -> Result<(), AhciError>;
pub enum AhciError { NotInitialized, PortError, Timeout }
```

Important properties of the current implementation (do not "fix" unless a step
requires it):

- `init()` finds a PCI device with class `0x01` / subclass `0x06`, reads BAR5 as
  the ABAR physical base, **identity-maps** the MMIO pages via
  `vmm::map_virtual_to_physical(phys, phys)`, enables AHCI mode, and rebases the
  first active SATA port (`DET==3 && IPM==1 && sig==SATA_SIG_ATA`).
- `port_rebase()` allocates **one** physical frame via `pmm`, identity-maps it,
  and lays out command list / FIS / command table / a **single 512-byte DMA
  buffer** inside it.
- `read_sectors()` issues `READ DMA EXT` (0x25) **one sector per command** in a
  loop, polling completion, and `copy_nonoverlapping`s each sector from the DMA
  buffer into the caller's buffer. `lba` is a `u32`.
- It is **poll-based** (no interrupts). Good — keep it that way.

`drivers/mod.rs` already declares `pub mod ahci;`.

### 2.2 The boot flow and the halt point

In `kernel/src/main.rs`:

- `drivers::pci::init()` runs before the halt block (PCI is already scanned).
- The UEFI/framebuffer detection is `booted_via_framebuffer(boot_info_raw, has_boot_info)`.
- The halt block (~line 215) prints a "UEFI Boot Successful / No legacy ATA disk
  detected / System halted." message via `console::with_console(...)` and then
  calls `idle_loop()` (never returns).
- **Below** that block, the legacy path runs: `drivers::ata::init()`,
  `io::fat12::init()`, IRQ handler registration, scheduler init, spawn keyboard
  worker, spawn shell via `process::exec_from_fat12("shell.bin")`, then
  `scheduler::wait_for_task_exit(shell_pid)`.

### 2.3 The user-space loader handoff (how we run the bytes)

`kernel/src/process/loader.rs` (re-exported through `kernel/src/process/mod.rs`):

```rust
pub fn load_program_image(file_name_8_3: &str) -> ExecResult<Vec<u8>>;      // FAT12 read
pub fn map_program_image_into_user_address_space(image: &[u8])             // map flat
    -> ExecResult<LoadedProgram>;                                          // binary -> user AS
pub fn exec_from_fat12(file_name_8_3: &str) -> ExecResult<usize>;          // legacy convenience
// private: fn spawn_loaded_program(loaded: LoadedProgram) -> ExecResult<usize>
```

`SHELL.BIN` is a **flat binary** (not ELF). The FAT12 path reads it into a
`Vec<u8>` and maps it. We will feed the FAT32-loaded `Vec<u8>` through the **same**
mapping path. To do that, add one public function (Step 4):

```rust
pub fn exec_from_image(image: &[u8]) -> ExecResult<usize> {
    let loaded = map_program_image_into_user_address_space(image)?; // validates length
    spawn_loaded_program(loaded)
}
```

and re-export it from `process/mod.rs`. `map_program_image_into_user_address_space`
already calls `validate_program_image_len(image.len())`, so no extra validation is
needed.

### 2.4 Helpers you may reuse

- `crate::io::fat12::normalize_8_3_name(name: &str) -> [u8; 11]` (re-exported)
  converts e.g. `"shell.bin"` to the 11-byte space-padded uppercase 8.3 form
  `b"SHELL   BIN"`. Reuse it to build the directory match key. **Verify its exact
  signature/return type before use** and adapt if needed.
- `crate::console::with_console(|c| { writeln!(c, ...) })` for framebuffer output.
- `crate::debugln!(...)` for serial debug output.
- `drivers::pci::find_by_class(class, subclass) -> Option<PciDevice>` (used by
  `ahci::init`).

### 2.5 The QEMU/test environment

The UEFI image is built by `build_uefi.sh` into `kaos64-uefi.img`: a GPT disk with
a **single FAT32 ESP** created at LBA `2048` (`sgdisk --new=1:2048:0 --typecode=1:ef00`),
currently populated with only `/EFI/BOOT/BOOTX64.EFI` and `/KERNEL.BIN` (via
`mtools` `mcopy ... ::/...`). It boots under QEMU **q35 + OVMF**, where the disk is
attached to the **ich9-AHCI** controller (the Proxmox VM script uses `--machine q35`
+ `--sata0`). So the same disk OVMF booted from is reachable via AHCI — exactly
what this slice needs.

## 3. Target architecture of the slice

```
UEFI boot (framebuffer + no ATA disk):
  pci::init()                         [already runs]
  ahci::init()                        [Step 1]
  gpt::find_esp_start_lba()           [Step 2]  -> partition base LBA
  fat32::Fat32Volume::mount(base)     [Step 3]  -> reads BPB, derives geometry
  vol.read_file("shell.bin")          [Step 3]  -> Vec<u8>
  process::exec_from_image(&bytes)    [Step 4]  -> spawn user-space shell
  scheduler bring-up + wait           [Step 4]
```

All new code reads the disk **only** through `drivers::ahci::read_sectors`. All
multi-byte on-disk integers are **little-endian**; decode with
`u16::from_le_bytes` / `u32::from_le_bytes` / `u64::from_le_bytes` from explicit
byte offsets (do **not** cast structs over raw buffers — alignment/packing risk).

Suggested new files:

- `kernel/src/io/gpt.rs`
- `kernel/src/io/fat32.rs` (single file is fine for this scope)

and `pub mod gpt; pub mod fat32;` in `kernel/src/io/mod.rs`.

---

## 4. Step 1 — Verify AHCI reads in isolation (de-risk)

**Why first:** the AHCI driver is unverified. Prove it reads *before* layering GPT
and FAT32 on top, so you never debug all three at once. The most likely failure is
the identity-mapping assumption (see §8).

**What to do:** in `main.rs`, inside the UEFI halt block (the one that currently
ends in `idle_loop()`), **before** halting:

1. Call `drivers::ahci::init()`.
2. Allocate a `let mut sector = [0u8; 512];` and call
   `drivers::ahci::read_sectors(&mut sector, 1, 1)`.
3. LBA 1 of a GPT disk is the **GPT header**, whose first 8 bytes are the ASCII
   signature `"EFI PART"` (`45 46 49 20 50 41 52 54`).
4. Print the outcome on the framebuffer console: the `Result`, and whether
   `&sector[0..8] == b"EFI PART"`. Also `debugln!` the first 16 bytes as hex.

**Acceptance:** booting `kaos64-uefi.img` under QEMU q35+OVMF shows
`read_sectors` returning `Ok` and the signature matching `EFI PART`. If it returns
`Err` or the signature is wrong, **stop and resolve §8 risks** before continuing —
the rest of the slice depends on this working.

> Keep this verification output behind the existing UEFI branch; it can stay as a
> `debugln!` once the full path works.

---

## 5. Step 2 — Parse the GPT to find the ESP (`io/gpt.rs`)

Implement:

```rust
/// Returns the starting LBA of the EFI System Partition, or None if not found.
pub fn find_esp_start_lba() -> Option<u64>;
```

### GPT header (read LBA 1, 512 bytes)

| Offset | Size | Field                      | Notes                         |
|--------|------|----------------------------|-------------------------------|
| 0x00   | 8    | Signature                  | must equal `b"EFI PART"`      |
| 0x48   | 8    | PartitionEntryLBA (u64 LE) | start of the entry array (=2) |
| 0x50   | 4    | NumberOfPartitionEntries   | u32 LE (e.g. 128)             |
| 0x54   | 4    | SizeOfPartitionEntry       | u32 LE (usually 128)          |

### GPT partition entry (in the array starting at PartitionEntryLBA)

| Offset | Size | Field                | Notes                          |
|--------|------|----------------------|--------------------------------|
| 0x00   | 16   | PartitionTypeGUID    | ESP GUID (see below)           |
| 0x20   | 8    | StartingLBA (u64 LE) | what we want                   |

A partition slot is **unused** if its PartitionTypeGUID is all zero — skip those.

### ESP type GUID

Canonical: `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`. On disk it is stored
**mixed-endian** (first three groups little-endian). Match against the exact
16-byte sequence:

```rust
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
    0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];
```

### Algorithm

1. Read LBA 1; verify `b"EFI PART"`. If mismatch, return `None`.
2. Read `entry_lba`, `num_entries`, `entry_size`.
3. `entries_per_sector = 512 / entry_size` (usually 4).
4. Iterate entries (cap the loop at `num_entries`, and defensively at e.g. 128).
   Read the sectors of the entry array via `read_sectors` (one or more sectors at
   a time). For each entry, compare bytes `[0x00..0x10]` to `ESP_TYPE_GUID`; on the
   first match return `StartingLBA` (bytes `[0x20..0x28]`).
5. If none matched, return `None`.

**Fallback (allowed, mark with `TODO`):** if parsing fails or finds nothing, you
may fall back to the known build-time ESP LBA `2048` (matches
`sgdisk --new=1:2048:0` in `build_uefi.sh`) so the slice still works while GPT
parsing is debugged. The real GPT path must be the primary one.

**Acceptance:** `find_esp_start_lba()` returns `Some(2048)` for the current
`build_uefi.sh` image. Log it with `debugln!`.

---

## 6. Step 3 — Minimal read-only FAT32 reader (`io/fat32.rs`)

Implement a small volume type that mounts the FAT32 partition at a given base LBA
and can read one named file from the **root directory**.

```rust
pub struct Fat32Volume {
    part_lba: u64,        // partition base (ESP StartingLBA)
    bytes_per_sec: u32,
    sec_per_clus: u32,
    fat_start_lba: u64,   // part_lba + reserved
    data_start_lba: u64,  // part_lba + reserved + num_fats * fat_size
    root_cluster: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum Fat32Error { Ahci, NotFat32, NotFound, IsDirectory, BadChain, TooLarge }

impl Fat32Volume {
    pub fn mount(part_lba: u64) -> Result<Self, Fat32Error>;
    pub fn read_file(&self, name: &str) -> Result<alloc::vec::Vec<u8>, Fat32Error>;
}
```

> `read_sectors` takes `lba: u32`. Cast `u64` LBAs to `u32` at the call site; this
> is fine for the small QEMU image. Note the `u32` ceiling as a known limitation.
> Assume `bytes_per_sec == 512` for this slice (assert / return `NotFat32`
> otherwise).

### 6.1 BPB (read the partition's first sector = LBA `part_lba`)

| Offset | Size | Field           | Use                                  |
|--------|------|-----------------|--------------------------------------|
| 0x0B   | 2    | BytsPerSec      | expect 512                           |
| 0x0D   | 1    | SecPerClus      | sectors per cluster                  |
| 0x0E   | 2    | RsvdSecCnt      | reserved sectors                     |
| 0x10   | 1    | NumFATs         | usually 2                            |
| 0x11   | 2    | RootEntCnt      | **must be 0** for FAT32              |
| 0x16   | 2    | FATSz16         | **must be 0** for FAT32              |
| 0x24   | 4    | FATSz32         | sectors per FAT                      |
| 0x2C   | 4    | RootClus        | first cluster of the root directory  |
| 0x1FE  | 2    | Signature       | `0x55 0xAA`                          |

Validate it is FAT32: `RootEntCnt == 0 && FATSz16 == 0` and the `0x55AA`
signature. Otherwise return `Fat32Error::NotFat32`.

Derive:

```
fat_start_lba  = part_lba + RsvdSecCnt
data_start_lba = part_lba + RsvdSecCnt + NumFATs * FATSz32
cluster_to_lba(n) = data_start_lba + (n - 2) * SecPerClus    // n >= 2
```

### 6.2 FAT32 chain walk

A FAT32 entry for cluster `n` is a little-endian `u32` at byte offset `n * 4`
within the FAT region:

```
sector  = fat_start_lba + (n * 4) / 512
offset  = (n * 4) % 512
value   = u32::from_le_bytes(sector[offset..offset+4]) & 0x0FFF_FFFF
```

- `value >= 0x0FFF_FFF8`  → end-of-chain (last cluster).
- `value == 0x0FFF_FFF7`  → bad cluster → `Fat32Error::BadChain`.
- `value < 2`             → invalid → `Fat32Error::BadChain`.
- otherwise               → next cluster index.

Guard against loops: cap the number of clusters followed (e.g. by a sane maximum
derived from file size, or a hard cap like 1<<20) and return `BadChain` if exceeded.

For this slice you may read the FAT one sector at a time per lookup (simple, slow,
correct). Optionally cache the last FAT sector read.

### 6.3 Directory entry (32 bytes each)

| Offset | Size | Field      | Notes                                            |
|--------|------|------------|--------------------------------------------------|
| 0x00   | 11   | Name (8.3) | space-padded, uppercase                          |
| 0x0B   | 1    | Attr       | `0x0F` = LFN entry (skip); `0x10` = directory     |
| 0x14   | 2    | FstClusHI  | u16 LE, high word of first cluster               |
| 0x1A   | 2    | FstClusLO  | u16 LE, low word of first cluster                |
| 0x1C   | 4    | FileSize   | u32 LE, bytes                                    |

- First byte `0x00` → no more entries in the directory (stop).
- First byte `0xE5` → deleted entry (skip).
- `Attr == 0x0F` → long-filename component (skip).
- `first_cluster = ((FstClusHI as u32) << 16) | (FstClusLO as u32)`.

### 6.4 `read_file` algorithm

1. Build the 8.3 match key: `let key = normalize_8_3_name(name);` (an
   `[u8; 11]`). Reuse `io::fat12::normalize_8_3_name` (verify signature) or
   inline a small equivalent.
2. Walk the **root-directory cluster chain** starting at `root_cluster`. For each
   cluster, read `sec_per_clus` sectors into a buffer; iterate its 32-byte entries:
   - byte0 `0x00` → not found, return `Fat32Error::NotFound`.
   - byte0 `0xE5` or `Attr == 0x0F` → skip.
   - if `entry[0..11] == key`:
     - if `Attr & 0x10 != 0` → `Fat32Error::IsDirectory`.
     - capture `first_cluster` and `file_size`; go to step 3.
   - advance to the next cluster via the FAT chain when the cluster's entries are
     exhausted.
3. Bound `file_size` (e.g. reject `> 8 MiB` → `Fat32Error::TooLarge`; `SHELL.BIN`
   is well under that). `Vec::with_capacity(file_size)`.
4. Walk the **file's** cluster chain from `first_cluster`: for each cluster read
   `sec_per_clus` sectors and append bytes, until `file_size` bytes have been
   collected or the chain ends. Truncate the final cluster to `file_size`.
5. Return the `Vec<u8>` (exactly `file_size` bytes).

**Acceptance:** `Fat32Volume::mount(esp_lba)` succeeds and reports plausible
geometry (`debugln!` the derived LBAs and `sec_per_clus`); `read_file("kernel.bin")`
returns a buffer whose length equals the on-disk `KERNEL.BIN` size (a good check
even before `SHELL.BIN` is added in Step 4, since `KERNEL.BIN` is already on the ESP).

---

## 7. Step 4 — Wire it in and run the shell

### 7.1 Put the user programs on the ESP

In `build_uefi.sh`, after the existing `mcopy` of `BOOTX64.EFI` and `KERNEL.BIN`,
add (at least) `SHELL.BIN`. Match the existing `mcopy -i "$IMG@@$PART_OFFSET"`
pattern and the legacy naming (`build.sh` uses `-n SHELL.BIN`, i.e. uppercase 8.3):

```sh
mcopy -i "$IMG@@$PART_OFFSET" "user_programs/shell/shell.bin" ::/SHELL.BIN
```

(Confirm the source path against `build.sh`, which copies the same artifacts for
the legacy image.) Copy any other programs the shell launches if needed; for the
first milestone `SHELL.BIN` alone is enough to prove the path.

### 7.2 Add the buffer-based exec entry point

In `kernel/src/process/loader.rs` add `exec_from_image` (see §2.3) and re-export it
from `kernel/src/process/mod.rs` alongside `exec_from_fat12`.

### 7.3 Restructure the boot tail in `main.rs`

Goal: both boot paths produce the shell image bytes, then share one scheduler
bring-up. Recommended shape (adapt to the existing code precisely):

- Compute `let uefi = booted_via_framebuffer(boot_info_raw, has_boot_info) && !drivers::ata::primary_present();`
- Obtain the shell image bytes:
  - **UEFI:**
    ```rust
    drivers::ahci::init();
    let esp = io::gpt::find_esp_start_lba().expect("ESP not found");
    let vol = io::fat32::Fat32Volume::mount(esp).expect("FAT32 mount failed");
    let shell_image = vol.read_file("shell.bin").expect("read SHELL.BIN failed");
    ```
  - **Legacy:** keep using the existing FAT12 init, then
    `let shell_image = process::load_program_image("shell.bin").expect(...);`
    (`drivers::ata::init()` + `io::fat12::init()` stay on this branch only.)
- Then a **shared** bring-up: register the keyboard IRQ handler, init + start the
  scheduler, spawn the keyboard worker task, then
  `let shell_pid = process::exec_from_image(&shell_image).expect(...);` and
  `scheduler::wait_for_task_exit(shell_pid as usize);`.
- Remove the `idle_loop()` dead-end from the UEFI branch.

> If a full restructure is too invasive, a pragmatic alternative is to **duplicate**
> the keyboard-IRQ + scheduler + spawn + wait block inside the UEFI branch (using
> `exec_from_image`) and keep the legacy fall-through untouched. Extracting a shared
> helper is cleaner but optional for this slice.

**Acceptance (end-to-end):** booting `kaos64-uefi.img` under QEMU q35+OVMF brings
up the interactive shell on the framebuffer console — the same shell the legacy
path runs — instead of halting. The legacy BIOS path (`kaos64.img`) is unchanged
and still works.

---

## 8. Risks & gotchas (read before Step 1)

1. **Identity mapping under the kernel's higher-half paging (most likely failure).**
   `ahci::init`/`port_rebase` assume `vmm::map_virtual_to_physical(phys, phys)`
   makes both the ABAR MMIO (often high, ~`0xFExx_xxxx`) and the DMA frame
   reachable at their physical address as a virtual address. The device DMAs to a
   **physical** address; the CPU reads the buffer through the identity VA — they
   must coincide. Verify `vmm::map_virtual_to_physical` accepts arbitrary physical
   addresses post-`ExitBootServices` and that the mapping is present (the driver
   guards with `vmm::is_va_mapped`). Step 1 surfaces this immediately. If it fails,
   inspect whether the ABAR page and the PMM frame are actually mapped, and whether
   the framebuffer-era identity window covers them.

2. **Endianness / struct casting.** Decode every on-disk field from explicit byte
   offsets with `from_le_bytes`. Do **not** overlay `#[repr(C)]` structs onto byte
   buffers (alignment + implicit assumptions bite here).

3. **`read_sectors` is one sector per command and `lba: u32`.** Correct but slow;
   fine for this slice. Keep cluster/FAT reads simple. Note the `u32` LBA ceiling.

4. **Cluster size.** Don't assume 1 sector per cluster — always read
   `sec_per_clus` sectors per cluster and size buffers accordingly.
   `sec_per_clus` fits in `u8`'s range for `read_sectors`' `sector_count` for
   typical FAT32 (≤ 128).

5. **AHCI port match.** `init` only accepts a port with `DET==3 && IPM==1` and
   `sig == SATA_SIG_ATA` (0x00000101). The q35 SATA disk should match; if `init`
   finds no drive, confirm the QEMU/Proxmox disk is on the AHCI (`sata`) controller,
   not virtio/IDE.

6. **8.3 name casing.** Directory names are uppercase, space-padded to 11 bytes
   with the extension right-aligned in the last 3 (`"SHELL   BIN"`). Build the key
   accordingly; `mcopy ::/SHELL.BIN` stores it uppercase.

7. **Don't regress the legacy path.** All new disk/FS selection must be gated on
   the UEFI branch. `drivers::ata::init()` / `io::fat12::init()` must still run for
   the BIOS boot.

## 9. Suggested commit breakdown

1. `feat(ahci): verify AHCI reads under UEFI (GPT signature probe)` — Step 1.
2. `feat(io): minimal GPT parser to locate the ESP` — Step 2 (`io/gpt.rs`).
3. `feat(io): minimal read-only FAT32 reader` — Step 3 (`io/fat32.rs`).
4. `feat(boot): load and run SHELL.BIN from ESP on the UEFI path` — Step 4
   (`exec_from_image`, `main.rs` wiring, `build_uefi.sh`).

## 10. Definition of done

- QEMU q35 + OVMF boots `kaos64-uefi.img` to the interactive shell (not a halt).
- The legacy BIOS image still boots to the shell unchanged.
- New code reads the disk only via `drivers::ahci::read_sectors`; `io::fat12` and
  the loaders are untouched; no AHCI write / no VFS trait introduced.
- Each step's acceptance check (§4–§7) passes and is observable via `debugln!`
  serial output or the framebuffer console.
