# Storage Abstraction Plan: ATA PIO (legacy BIOS) + AHCI (UEFI)

**Status:** Design / ready for implementation
**Branch:** `feature/ahci-driver`
**Audience:** the coding AI that will implement this. Read the whole document before
touching code. File paths are relative to `main64/` unless noted otherwise.

---

## 1. Problem statement

`main.rs` has two boot paths that both successfully load and run `shell.bin`:

* **Legacy BIOS** — loads `shell.bin` via `process::load_program_image()` →
  `io::fat12::read_file()` → **hardcoded** `drivers::ata::read_sectors()`.
* **UEFI** — loads `shell.bin` via `io::fat32::Fat32Volume::mount()` +
  `read_file()` → **hardcoded** `crate::drivers::ahci::read_sectors()`.

The bug: once `shell.bin` runs and issues a **file-access syscall** (`dir`, `cat`,
exec another program), control *always* lands in the FAT12 + ATA PIO path,
regardless of how we booted:

* `syscall/dispatch/fs.rs` → `crate::io::fat12::{open_file, read_file_fd, …}`
  → `io::fat12/*` → `drivers::ata::{read,write}_sectors`.
* `process/loader.rs:63` → `fat12::read_file()` → `drivers::ata::read_sectors`.

On UEFI/AHCI hardware there is **no working legacy ATA controller**, so every
in-shell file access fails.

There are therefore **two** independent couplings to break:

1. **Block-device coupling** — FAT12 hardcodes ATA; FAT32 hardcodes AHCI.
2. **Filesystem coupling** — the syscall layer and the loader only understand
   FAT12, while the UEFI volume is FAT32.

This plan fixes **both**: a `BlockDevice` abstraction (ATA *or* AHCI, chosen once
at boot) plus a `FileSystem` abstraction (FAT12 *or* FAT32, chosen once at boot)
behind a single VFS facade that the syscall layer and loader call.

**Scope decision (confirmed):**
* **AHCI is read-only for now.** AHCI `write_sectors` is *out of scope*. Write
  syscalls (`write`, `delete`, save) must keep working on the BIOS/FAT12 path and
  must fail **gracefully** with a clear "unsupported" error on the UEFI/FAT32 path
  — never panic, never corrupt.
* Build a `FileSystem` trait now (FAT12 + FAT32 adapters), not a full mount-table
  VFS. A single global mounted filesystem is sufficient.

---

## 2. Current-state reference (verified)

Block drivers (both 512-byte sectors, both already exist):

| Driver | File | Read | Write | Global state |
|---|---|---|---|---|
| ATA PIO | `kernel/src/drivers/ata.rs` | `read_sectors(buf: &mut [u8], lba: u32, count: u8) -> Result<(), AtaError>` (L374) | `write_sectors(buf: &[u8], lba: u32, count: u8) -> Result<(), AtaError>` (L435) | `PRIMARY_ATA`, IRQ14 waitqueue. `init()` L356, `primary_present()` L348 |
| AHCI | `kernel/src/drivers/ahci.rs` | `read_sectors(buf: &mut [u8], lba: u32, count: u8) -> Result<(), AhciError>` (L576) | — none — | `static mut ACTIVE_PORT`, `DMA_BUFFER_PHYS`. `init()` L183. `AhciError { NotInitialized, PortError, Timeout }` L570 |

Filesystems:

* `kernel/src/io/fat12/` — `disk.rs` (root/FAT sector I/O at fixed LBAs),
  `fd.rs` (FD table `FILE_DESCRIPTORS`, `open_file`/`read_file_fd`/`write_file_fd`/
  `seek_file`/`eof_file`/`close_file`), `fs.rs` (`read_file`, `read_file_from_entry`,
  `print_root_directory`, `FileMode { Read, Write, Append }`), `cluster.rs`,
  `directory.rs`, `types.rs` (`Fat12Error::Ata(AtaError)` L53). All sector I/O is
  `drivers::ata::{read,write}_sectors`.
* `kernel/src/io/fat32.rs` — `Fat32Volume::mount(part_lba: u64)` (L56),
  `read_file(&self, name) -> Result<Vec<u8>, Fat32Error>` (L118). Read-only, no FD
  layer. All sector I/O is `crate::drivers::ahci::read_sectors` (L62/144/224/269,
  one sector per call). `Fat32Error::Ahci` L33.

Consumers to rewire:

* `kernel/src/syscall/dispatch/fs.rs` — handlers call `crate::io::fat12::*`
  (`open_file` L50, `read_file_fd` L73, `write_file_fd` L90, `delete_file` L97,
  `seek_file` L103, `eof_file` L110, `print_root_directory` L116, `close_file` L56).
* `kernel/src/process/loader.rs:63` — `fat12::read_file(file_name_8_3)`.
* `kernel/src/main.rs` — boot-path selection around L231–280
  (`let uefi = booted_via_framebuffer(...) && !drivers::ata::primary_present();`),
  UEFI branch mounts FAT32 and calls `vol.read_file("shell.bin")`; BIOS branch
  calls `process::load_program_image("shell.bin")`.

Module roots: `drivers/mod.rs` declares the driver modules; `io/mod.rs` declares
`fat12`, `fat32`, `gpt`.

---

## 3. Target architecture

```
 syscall/dispatch/fs.rs        process/loader.rs        main.rs (boot)
            \                        |                       /
             \                       v                      /
              ----------->   io::vfs  (FileSystem facade) <-
                                   |   global: MOUNTED_FS
                     +-------------+-------------+
                     |                           |
              Fat12Fs (adapter)           Fat32Fs (adapter, read-only)
                     |                           |
                     +-------------+-------------+
                                   |
                          drivers::block (BlockDevice facade)
                                   |   global: ACTIVE_DEVICE
                       +-----------+-----------+
                       |                       |
                AtaBlockDevice          AhciBlockDevice
                       |                       |
              drivers::ata            drivers::ahci
```

Two new facades, two globals, each selected **once** at boot:

* `drivers::block` — `BlockDevice` trait + ATA/AHCI adapters + `ACTIVE_DEVICE`.
* `io::vfs` — `FileSystem` trait + FAT12/FAT32 adapters + `MOUNTED_FS`.

Filesystems stop calling drivers directly; they call `drivers::block`. The syscall
layer and loader stop calling `io::fat12` directly; they call `io::vfs`.

---

## 4. Phase 1 — `BlockDevice` abstraction

### 4.1 New file `kernel/src/drivers/block.rs`

```rust
//! Block-device abstraction: a single 512-byte-sector device selected at boot.
//! Filesystems call this facade instead of a concrete driver, so the same FS
//! code runs over ATA PIO (legacy BIOS) or AHCI (UEFI).

use crate::drivers::{ahci, ata};
use crate::sync::SpinLock; // confirm actual SpinLock path/import used elsewhere

/// Fixed sector size for every supported device (matches ATA + AHCI).
pub const SECTOR_SIZE: usize = 512;

/// Maximum sectors transferable in one hardware command (ATA/AHCI count is u8).
const MAX_SECTORS_PER_CMD: u32 = 255;

/// Highest LBA addressable by 28-bit ATA PIO.
const ATA_MAX_LBA: u64 = 0x0FFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// No device selected (init_* never called) or device not ready.
    NotReady,
    /// Caller buffer smaller than count * SECTOR_SIZE.
    BadBuffer,
    /// LBA exceeds what the active device can address.
    OutOfRange,
    /// Underlying driver failed (carries which backend for diagnostics).
    Device,
    /// Operation not supported by the active device (e.g. AHCI writes).
    Unsupported,
}

/// One 512-byte-sector block device. lba/count are u64/u32 so the trait outlives
/// 28-bit ATA; adapters clamp/chunk to their hardware limits.
pub trait BlockDevice: Send + Sync {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_sectors(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError>;
    fn sector_size(&self) -> usize { SECTOR_SIZE }
}

// ---- ATA adapter (read + write) ----------------------------------------------
pub struct AtaBlockDevice;

impl BlockDevice for AtaBlockDevice {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        check_buf(buf.len(), count)?;
        chunked(lba, count, ATA_MAX_LBA, |chunk_lba, chunk_cnt, off| {
            let bytes = chunk_cnt as usize * SECTOR_SIZE;
            ata::read_sectors(&mut buf[off..off + bytes], chunk_lba as u32, chunk_cnt as u8)
                .map_err(|_| BlockError::Device)
        })
    }
    fn write_sectors(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError> {
        check_buf(buf.len(), count)?;
        chunked(lba, count, ATA_MAX_LBA, |chunk_lba, chunk_cnt, off| {
            let bytes = chunk_cnt as usize * SECTOR_SIZE;
            ata::write_sectors(&buf[off..off + bytes], chunk_lba as u32, chunk_cnt as u8)
                .map_err(|_| BlockError::Device)
        })
    }
}

// ---- AHCI adapter (read-only for now) ----------------------------------------
pub struct AhciBlockDevice;

impl BlockDevice for AhciBlockDevice {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        check_buf(buf.len(), count)?;
        // AHCI read_sectors currently takes lba: u32. Keep the u32 ceiling until
        // 48-bit LBA is wired in the driver.
        chunked(lba, count, u32::MAX as u64, |chunk_lba, chunk_cnt, off| {
            let bytes = chunk_cnt as usize * SECTOR_SIZE;
            ahci::read_sectors(&mut buf[off..off + bytes], chunk_lba as u32, chunk_cnt as u8)
                .map_err(|_| BlockError::Device)
        })
    }
    fn write_sectors(&self, _lba: u64, _count: u32, _buf: &[u8]) -> Result<(), BlockError> {
        // Out of scope: AHCI is read-only in this iteration.
        Err(BlockError::Unsupported)
    }
}

fn check_buf(buf_len: usize, count: u32) -> Result<(), BlockError> {
    if buf_len < count as usize * SECTOR_SIZE { Err(BlockError::BadBuffer) } else { Ok(()) }
}

/// Split a multi-sector request into <=255-sector hardware commands, enforcing
/// the device's LBA ceiling. `op(lba, count, byte_offset)` does one command.
fn chunked(
    lba: u64, count: u32, max_lba: u64,
    mut op: impl FnMut(u64, u32, usize) -> Result<(), BlockError>,
) -> Result<(), BlockError> {
    if count == 0 { return Ok(()); }
    if lba.checked_add(count as u64 - 1).map_or(true, |last| last > max_lba) {
        return Err(BlockError::OutOfRange);
    }
    let mut remaining = count;
    let mut cur = lba;
    let mut off = 0usize;
    while remaining > 0 {
        let n = remaining.min(MAX_SECTORS_PER_CMD);
        op(cur, n, off)?;
        cur += n as u64;
        off += n as usize * SECTOR_SIZE;
        remaining -= n;
    }
    Ok(())
}

// ---- Global selected device --------------------------------------------------
static ATA_DEVICE: AtaBlockDevice = AtaBlockDevice;
static AHCI_DEVICE: AhciBlockDevice = AhciBlockDevice;

static ACTIVE_DEVICE: SpinLock<Option<&'static dyn BlockDevice>> = SpinLock::new(None);

/// Select ATA PIO as the active block device. Call after `ata::init()`.
pub fn init_ata() { *ACTIVE_DEVICE.lock() = Some(&ATA_DEVICE); }

/// Select AHCI as the active block device. Call after `ahci::init()`.
pub fn init_ahci() { *ACTIVE_DEVICE.lock() = Some(&AHCI_DEVICE); }

/// Read `count` sectors at `lba` into `buf` from the active device.
pub fn read_sectors(lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
    let dev = (*ACTIVE_DEVICE.lock()).ok_or(BlockError::NotReady)?;
    dev.read_sectors(lba, count, buf)
}

/// Write `count` sectors at `lba`. Errors with `Unsupported` on read-only devices.
pub fn write_sectors(lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError> {
    let dev = (*ACTIVE_DEVICE.lock()).ok_or(BlockError::NotReady)?;
    dev.write_sectors(lba, count, buf)
}
```

Implementation notes for the coder:
* **Verify the `SpinLock` import path and API** against `kernel/src/sync/` (the
  memory index says `src/sync/spinlock.rs`; match how other modules import it).
  `lock()` should return a guard that derefs to `Option<&'static dyn BlockDevice>`.
* Storing a `&'static dyn BlockDevice` (a fat pointer) inside a `SpinLock<Option<…>>`
  avoids `AtomicPtr` fat-pointer problems and needs no `unsafe`. If the existing
  code style prefers a `static mut` set-once-before-interrupts pattern (as AHCI's
  `ACTIVE_PORT` does), that is acceptable too — but the `SpinLock` version is
  cleaner and matches the kernel's stated preference for `SpinLock` over `static mut`.
* `read_sectors`/`write_sectors` take the lock only to copy out the `&'static`
  reference, then release it before the (potentially yielding) driver call. Do
  **not** hold the lock across the driver call — ATA yields to the scheduler.

### 4.2 Register the module

In `kernel/src/drivers/mod.rs` add `pub mod block;` (alphabetical: before `keyboard`).

---

## 5. Phase 2 — Route filesystems through `drivers::block`

Replace every direct driver call inside the filesystems with the block facade.
This is what literally fixes "storage access over both ATA and AHCI": after this
phase a filesystem no longer knows or cares which controller is underneath.

### 5.1 FAT12 (`kernel/src/io/fat12/`)

Replace these call sites:

| File:line | From | To |
|---|---|---|
| `disk.rs:17` | `drivers::ata::read_sectors(&mut buffer, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)?` | `crate::drivers::block::read_sectors(ROOT_DIRECTORY_LBA as u64, ROOT_DIRECTORY_SECTORS as u32, &mut buffer)?` |
| `disk.rs:25` | `read_sectors(&mut buffer, FAT1_LBA, SECTORS_PER_FAT as u8)` | `block::read_sectors(FAT1_LBA as u64, SECTORS_PER_FAT as u32, &mut buffer)` |
| `disk.rs:38` | `write_sectors(buffer, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)` | `block::write_sectors(ROOT_DIRECTORY_LBA as u64, ROOT_DIRECTORY_SECTORS as u32, buffer)` |
| `disk.rs:52` / `disk.rs:55` | `write_sectors(fat_buffer, FAT1_LBA …)` / `… fat2_lba …` | `block::write_sectors(…)` |
| `fs.rs:77` | `drivers::ata::read_sectors(&mut sector, cluster_lba, 1)` | `block::read_sectors(cluster_lba as u64, 1, &mut sector)` |
| `fd.rs:209` / `fd.rs:281` | `read_sectors(&mut temp_buffer, cluster_lba, 1)` | `block::read_sectors(cluster_lba as u64, 1, &mut temp_buffer)` |
| `fd.rs:286` | `write_sectors(&temp_buffer, cluster_lba, 1)` | `block::write_sectors(cluster_lba as u64, 1, &temp_buffer)` |
| `cluster.rs:95` / `cluster.rs:119` | `write_sectors(&empty_sector, cluster_lba, 1)` | `block::write_sectors(cluster_lba as u64, 1, &empty_sector)` |

Note the **argument-order change**: block facade is `(lba, count, buf)`, the old
drivers were `(buf, lba, count)`. Do not blindly swap; re-read each call.

Error handling (`types.rs`): change `Fat12Error::Ata(AtaError)` to wrap
`BlockError` instead. Replace the variant with `Fat12Error::Block(BlockError)` and
update the `From<AtaError>` impl (L74) to `From<BlockError>`. Update the `Display`
arm (L84) accordingly. The `?` operators at the call sites then need
`From<BlockError> for Fat12Error`.

### 5.2 FAT32 (`kernel/src/io/fat32.rs`)

| Line | From | To |
|---|---|---|
| 62 | `crate::drivers::ahci::read_sectors(&mut sector, part_lba as u32, 1)` | `crate::drivers::block::read_sectors(part_lba, 1, &mut sector)` |
| 143/144 | `ahci::read_sectors(&mut sector, (cluster_lba + i) as u32, 1)` | `block::read_sectors(cluster_lba + i as u64, 1, &mut sector)` |
| 223/224 | same pattern | `block::read_sectors(cluster_lba + i as u64, 1, &mut sector)` |
| 268/269 | `ahci::read_sectors(&mut sector, fat_sector as u32, 1)` | `block::read_sectors(fat_sector, 1, &mut sector)` |

Error handling: replace `Fat32Error::Ahci` with `Fat32Error::Block(BlockError)`
(or keep a unit `Block` variant if the existing code only does
`.map_err(|_| Fat32Error::Ahci)` — minimal change: rename `Ahci` → `Block` and map
`|_| Fat32Error::Block`). Keep it simple; the variant name is the only churn.

**Optional cleanup (not required):** FAT32 reads one sector per call in loops. Now
that `block::read_sectors` chunks internally, the per-cluster loops *could* read
`sec_per_clus` sectors in one call. Leave as-is unless trivial; correctness first.

After Phase 2: build must still pass. BIOS boot is functionally unchanged (ATA
selected → FAT12 over ATA). UEFI is unchanged so far because `main.rs` still calls
`fat32::read_file` directly — we wire selection in Phase 4.

---

## 6. Phase 3 — `FileSystem` abstraction + adapters

### 6.1 New file `kernel/src/io/vfs.rs`

A thin facade over a single mounted filesystem. FD-style API mirrors what the
syscall layer already expects, so the syscall handlers barely change.

```rust
//! Single-mount filesystem facade. One filesystem (FAT12 or FAT32) is mounted at
//! boot; syscalls and the program loader call this instead of a concrete FS.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::SpinLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotMounted,
    NotFound,
    InvalidFd,
    /// Operation not supported by the mounted FS (e.g. writes on read-only FAT32).
    Unsupported,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode { Read, Write, Append }

/// Operations the syscall layer + loader need. Writes may return `Unsupported`.
pub trait FileSystem: Send + Sync {
    fn open(&self, name: &str, mode: FileMode) -> Result<usize, FsError>;
    fn close(&self, fd: usize) -> Result<(), FsError>;
    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize, FsError>;
    fn seek(&self, fd: usize, offset: u32) -> Result<(), FsError>;
    fn eof(&self, fd: usize) -> Result<bool, FsError>;
    fn delete(&self, name: &str) -> Result<(), FsError>;
    /// Whole-file read for the program loader.
    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError>;
    fn print_root_directory(&self);
}

static MOUNTED_FS: SpinLock<Option<Box<dyn FileSystem>>> = SpinLock::new(None);

pub fn mount(fs: Box<dyn FileSystem>) { *MOUNTED_FS.lock() = Some(fs); }

// Facade helpers used by syscalls/loader. Each locks, dispatches, unlocks.
// NOTE: these hold the MOUNTED_FS lock across the FS call. Confirm the FS impls
// do not themselves try to re-lock MOUNTED_FS (they must not). If FS calls can
// yield (they do disk I/O via the block layer, and ATA yields), holding a
// SpinLock that disables interrupts across a yield is WRONG. See 6.4.
pub fn open(name: &str, mode: FileMode) -> Result<usize, FsError> { with(|fs| fs.open(name, mode)) }
pub fn close(fd: usize) -> Result<(), FsError> { with(|fs| fs.close(fd)) }
pub fn read(fd: usize, buf: &mut [u8]) -> Result<usize, FsError> { with(|fs| fs.read(fd, buf)) }
pub fn write(fd: usize, buf: &[u8]) -> Result<usize, FsError> { with(|fs| fs.write(fd, buf)) }
pub fn seek(fd: usize, off: u32) -> Result<(), FsError> { with(|fs| fs.seek(fd, off)) }
pub fn eof(fd: usize) -> Result<bool, FsError> { with(|fs| fs.eof(fd)) }
pub fn delete(name: &str) -> Result<(), FsError> { with(|fs| fs.delete(name)) }
pub fn read_file(name: &str) -> Result<Vec<u8>, FsError> { with(|fs| fs.read_file(name)) }
pub fn print_root_directory() { let _ = with(|fs| { fs.print_root_directory(); Ok(()) }); }
```

### 6.2 Concurrency: do **not** hold a `SpinLock` across disk I/O

This is the single most important correctness constraint. The kernel's `SpinLock`
**disables interrupts on lock and the ATA path yields to the scheduler** while
waiting on IRQ14. Holding `MOUNTED_FS` (or `ACTIVE_DEVICE`) across a yielding disk
read would deadlock or run with interrupts disabled across a context switch.

Resolve with the `with(...)` helper that copies out a stable reference and
releases the lock before calling into the FS:

```rust
fn with<R>(f: impl FnOnce(&dyn FileSystem) -> Result<R, FsError>) -> Result<R, FsError> {
    // The mounted FS is set once at boot and never replaced, so a raw pointer
    // captured under the lock stays valid for the kernel's lifetime.
    let ptr: *const dyn FileSystem = {
        let guard = MOUNTED_FS.lock();
        match guard.as_deref() {
            Some(fs) => fs as *const dyn FileSystem,
            None => return Err(FsError::NotMounted),
        }
    }; // lock released here
    // SAFETY: MOUNTED_FS is mount-once at boot; the Box is never dropped/replaced,
    // so the pointer remains valid. No &mut aliasing: FileSystem uses interior
    // mutability (its own SpinLock'd FD table), all methods take &self.
    f(unsafe { &*ptr })
}
```

Same rule already applied to `drivers::block` in Phase 1 (lock only to copy the
`&'static dyn` out). Document both with `SAFETY:` comments per kernel convention.

### 6.3 FAT12 adapter — `Fat12Fs`

Lowest-risk: wrap the **existing** `io::fat12` functions (which already own the
`FILE_DESCRIPTORS` table and work today). Put the adapter in
`kernel/src/io/fat12/mod.rs` (or a new `fat12/vfs_impl.rs`).

```rust
pub struct Fat12Fs;

impl crate::io::vfs::FileSystem for Fat12Fs {
    fn open(&self, name, mode) -> ... { fat12::open_file(name, map_mode(mode)).map_err(map_err) }
    fn read(&self, fd, buf)    -> ... { fat12::read_file_fd(fd, buf).map_err(map_err) }
    fn write(&self, fd, buf)   -> ... { fat12::write_file_fd(fd, buf).map_err(map_err) }
    fn seek(&self, fd, off)    -> ... { fat12::seek_file(fd, off).map_err(map_err) }
    fn eof(&self, fd)          -> ... { fat12::eof_file(fd).map_err(map_err) }
    fn close(&self, fd)        -> ... { fat12::close_file(fd).map_err(map_err) }
    fn delete(&self, name)     -> ... { fat12::delete_file(name).map_err(map_err) }
    fn read_file(&self, name)  -> ... { fat12::read_file(name).map_err(map_err) }
    fn print_root_directory(&self)   { fat12::print_root_directory() }
}
```

* `map_mode`: `vfs::FileMode` → `fat12::FileMode` (same three variants).
* `map_err`: `Fat12Error` → `FsError` (`NotFound`/`InvalidFd`/`Io`). Keep it total.
* Confirm the exact signatures/return types of the `fat12::*` functions and adjust
  (e.g. `read_file_fd` returns `Result<usize, Fat12Error>` per `fd.rs:172`).

### 6.4 FAT32 adapter — `Fat32Fs` (read-only)

FAT32 has no FD layer. Add a minimal read-only FD table inside
`kernel/src/io/fat32.rs` (or `fat32_vfs.rs`). The simplest correct design that
reuses the existing `Fat32Volume::read_file`:

```rust
struct Fat32OpenFile { name: String, data: Vec<u8>, offset: usize } // see note

pub struct Fat32Fs {
    volume: Fat32Volume,
    open_files: SpinLock<Vec<Option<Fat32OpenFile>>>,
}
```

Two implementation options — pick **A** unless memory is a concern:

* **Option A (eager, simplest):** `open()` calls `volume.read_file(name)` once and
  caches the whole `Vec<u8>` in the FD. `read()` copies the next slice from the
  cached buffer and advances `offset`. `seek()` sets `offset`. `eof()` is
  `offset >= data.len()`. `close()` drops the entry. This reuses the already-working
  `read_file` cluster walk verbatim — least new code, least risk. Cost: whole file
  in RAM (fine for shell-loaded programs and `cat` of small files).
* **Option B (streaming):** store cluster-chain cursor like FAT12 and read on
  demand. More code, more risk; only do this if large files matter. Not recommended
  for the first iteration.

Write-side methods return `Err(FsError::Unsupported)`:
```rust
fn write(&self, _, _) -> Result<usize, FsError> { Err(FsError::Unsupported) }
fn delete(&self, _)   -> Result<(), FsError>    { Err(FsError::Unsupported) }
```
`read_file(name)` delegates straight to `self.volume.read_file(name)`.
`print_root_directory()` — implement a FAT32 root-dir listing (the volume already
walks the root cluster chain in `read_file`; factor out a directory-entry iterator
and print names). If time-boxed, a minimal listing is acceptable; do **not** leave
it calling FAT12.

FD-table concurrency: the inner `SpinLock<Vec<…>>` must follow the same
"don't hold across disk I/O" rule. With Option A, the only disk I/O is inside
`open()`'s `read_file`; structure it as: read file (no lock) → then lock briefly to
insert the FD entry. `read`/`seek`/`eof` touch only RAM, so locking is fine.

### 6.5 Register modules

`io/mod.rs`: add `pub mod vfs;`. Keep `fat12`/`fat32`/`gpt`.

---

## 7. Phase 4 — Rewire consumers to the VFS

### 7.1 `kernel/src/syscall/dispatch/fs.rs`

Swap every `crate::io::fat12::X` for `crate::io::vfs::X`:

| Handler | From | To |
|---|---|---|
| `syscall_open_file_impl` (L50) | `io::fat12::open_file(&name, mode)` | `io::vfs::open(&name, mode)` |
| mode mapping (L44–46) | `io::fat12::FileMode` | `io::vfs::FileMode` |
| `syscall_close_file_impl` (L56) | `io::fat12::close_file` | `io::vfs::close` |
| `syscall_read_file_impl` (L73) | `io::fat12::read_file_fd` | `io::vfs::read` |
| `syscall_write_file_impl` (L90) | `io::fat12::write_file_fd` | `io::vfs::write` |
| `syscall_delete_file_impl` (L97) | `io::fat12::delete_file` | `io::vfs::delete` |
| `syscall_seek_file_impl` (L103) | `io::fat12::seek_file` | `io::vfs::seek` |
| `syscall_end_of_file_impl` (L110) | `io::fat12::eof_file` | `io::vfs::eof` |
| `syscall_print_root_directory_impl` (L116) | `io::fat12::print_root_directory()` | `io::vfs::print_root_directory()` |

Map `FsError` → existing `SyscallError` (`Io`, `InvalidArg`, …). Critically:
`FsError::Unsupported` should map to a sensible syscall error (e.g. `Io` or a new
`Unsupported`) so a `write`/`delete` on the UEFI/FAT32 mount fails cleanly and the
shell can report it — **no panic**.

### 7.2 `kernel/src/process/loader.rs`

* Line 5 `use crate::io::fat12::{self, Fat12Error};` → use `crate::io::vfs`.
* Line 63 `fat12::read_file(file_name_8_3)` → `vfs::read_file(file_name_8_3)`.
* `map_fat12_error` → `map_fs_error` mapping `FsError` → `ExecError`. Keep total.

This makes "exec another program from the shell" work on both boot paths.

### 7.3 `kernel/src/main.rs` boot wiring

Around L231–280, after the `uefi` decision, select the block device **and** mount
the filesystem, then load `shell.bin` through the unified loader for both paths:

```rust
let shell_image = if uefi {
    drivers::ahci::init();
    drivers::block::init_ahci();                       // NEW: select AHCI
    let esp_lba = io::gpt::find_esp_start_lba().expect("ESP not found on GPT disk");
    let vol = io::fat32::Fat32Volume::mount(esp_lba).expect("FAT32 ESP mount failed");
    io::vfs::mount(alloc::boxed::Box::new(io::fat32::Fat32Fs::new(vol)));  // NEW
    io::vfs::read_file("shell.bin").expect("failed to read SHELL.BIN from ESP")
} else {
    drivers::ata::init();
    drivers::block::init_ata();                        // NEW: select ATA
    io::fat12::init();
    io::vfs::mount(alloc::boxed::Box::new(io::fat12::Fat12Fs));            // NEW
    io::vfs::read_file("shell.bin").expect("failed to load SHELL.BIN from FAT12")
};
```

Both branches now: (1) init driver, (2) select block device, (3) mount FS, (4)
read `shell.bin` via the VFS. `gpt::find_esp_start_lba()` already uses the AHCI
read path — after Phase 2 it goes through `drivers::block`, so it must run **after**
`init_ahci()`. Verify `gpt.rs` call sites and ordering.

Note the change from `process::load_program_image` to `io::vfs::read_file` for the
BIOS branch — the loader's `load_program_image` now also routes through the VFS
(7.2), so either entry point works; keep them consistent.

---

## 8. Phase 5 — Error mapping, graceful write failure, cleanup

* Ensure all new error enums derive `Debug, Clone, Copy, PartialEq, Eq`.
* Provide total `From`/`map_*` conversions: `BlockError → Fat12Error/Fat32Error`,
  `Fat12Error/Fat32Error → FsError`, `FsError → SyscallError`/`ExecError`.
* Write/delete on FAT32 mount: confirm end-to-end that the syscall returns an error
  the shell surfaces (test by attempting a write under UEFI) — must not panic.
* Remove now-dead direct driver imports from `io::fat12::*` and `io::fat32`
  (`use crate::drivers::ata`, `…::ahci`) once all call sites are migrated.
* Keep `drivers::ata` / `drivers::ahci` public — they are still the backends.

---

## 9. Build & test verification

Build both profiles (the repo has `build_kaos_debug.sh`, `build_kaos_release.sh`,
`build_uefi.sh`; kernel crate is `main64/kernel`):

```
# from main64/
cargo build            # workspace builds; or per the existing build scripts
cargo test -p kernel   # unit/integration tests; note ahci_test.rs exists
```

Manual boot tests (scripts present: `kaos64-bios-vm.sh`, `kaos64-uefi-vm.sh`):

1. **Legacy BIOS (QEMU, ATA):** boot, run `dir`, `cat <file>`, exec another
   program from the shell, and a `write`/`delete` if supported. All must behave
   exactly as before this change (regression check).
2. **UEFI (QEMU, AHCI):** boot, run `dir`, `cat <file>`, exec another program.
   These previously failed — they must now succeed via FAT32-over-AHCI.
3. **UEFI write attempt:** attempt a write/delete; expect a clean error in the
   shell, no panic, no hang.
4. If real-HW (AMD/Proxmox q35) testing is in reach, repeat (2) there — see the
   project memory on AHCI multi-controller / COMRESET quirks.

Acceptance criteria:
* No direct `drivers::ata::*` / `drivers::ahci::*` calls remain in `io/` (grep).
* No syscall/loader code references `io::fat12` directly (all via `io::vfs`).
* BIOS path: unchanged behavior. UEFI path: `dir`/`cat`/exec work.
* No `SpinLock` is held across a disk read anywhere (block + vfs facades both copy
  the reference out before calling). Audit per §6.2.

---

## 10. Risk & sequencing notes

* **Lock-across-yield is the top hazard.** ATA `read_sectors` yields while waiting
  on IRQ14; the kernel `SpinLock` disables interrupts. The `with()`/copy-out
  pattern in §6.2 / Phase 1 is mandatory, not stylistic.
* **Argument-order flip** `(buf, lba, count)` → `(lba, count, buf)` is an easy
  silent bug. Migrate one call site at a time and re-read each.
* **Phase ordering matters:** `block::init_ahci()` must precede the first
  `gpt`/`fat32` read; `vfs::mount` must precede the first `vfs::read_file`.
* **Keep BIOS path bit-for-bit behavioral.** FAT12 adapter is a thin wrapper over
  the existing, working functions — do not rewrite FAT12 internals.
* **AHCI writes deliberately deferred.** When implemented later, only
  `AhciBlockDevice::write_sectors` and `Fat32Fs` write methods change; the VFS and
  syscall layers already support writes.
* **Out of scope:** multi-mount VFS / mount table, 48-bit LBA in AHCI, interrupt-
  driven AHCI, NVMe. The trait shapes leave room for all of these.

---

## 11. File-change checklist

New:
* `kernel/src/drivers/block.rs` — `BlockDevice`, `Ata/AhciBlockDevice`, `ACTIVE_DEVICE`, facade.
* `kernel/src/io/vfs.rs` — `FileSystem`, `FsError`, `FileMode`, `MOUNTED_FS`, facade.
* `Fat32Fs` adapter (in `fat32.rs` or `fat32_vfs.rs`) + read-only FD table.
* `Fat12Fs` adapter (in `fat12/mod.rs` or `fat12/vfs_impl.rs`).

Edited:
* `kernel/src/drivers/mod.rs` — `pub mod block;`.
* `kernel/src/io/mod.rs` — `pub mod vfs;`.
* `kernel/src/io/fat12/{disk.rs, fs.rs, fd.rs, cluster.rs, types.rs}` — route to
  `drivers::block`; `Fat12Error::Block`.
* `kernel/src/io/fat32.rs` — route to `drivers::block`; `Fat32Error::Block`.
* `kernel/src/syscall/dispatch/fs.rs` — call `io::vfs::*`; map `FsError`.
* `kernel/src/process/loader.rs` — `vfs::read_file`; map `FsError`.
* `kernel/src/main.rs` — `block::init_{ata,ahci}` + `vfs::mount` in both branches.
```
