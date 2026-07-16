# KAOS Rust Kernel: Storage Subsystem

This document explains how KAOS reads and writes disks, end to end: from the raw
hardware controllers (ATA PIO and AHCI/SATA), up through a block-device
abstraction, partition discovery (GPT), the FAT32 filesystem, and the
single-mount Virtual File System (VFS) facade that the syscall layer and program
loader call. It covers both the Rust architecture and the hardware-level protocol
details behind each driver.

A developer who has never touched this code should, after reading, understand the
full path a `cat foo.txt` from the shell takes — from the syscall down to the
specific I/O ports or MMIO registers that move the bytes — and *why* the layering
exists.

> **Companion document.** `docs/storage_abstraction.md` is the *design plan* that
> motivated this architecture (the "fix in-shell file access under UEFI" effort).
> This document describes the code **as built**. Where they disagree, the code
> (and this document) win.

---

## 1. The Problem: Two Controllers × Two Filesystems

KAOS boots two ways, and the two paths historically reached storage through
completely different, hardcoded stacks:

- **Legacy BIOS boot** has a legacy IDE/ATA disk. The boot floppy/disk image is
  formatted **FAT32** and is read via the **VFS**.
- **UEFI boot** has no legacy ATA controller. The disk is a GPT-partitioned SATA
  device reached through an **AHCI** controller, and the EFI System Partition is
  formatted **FAT32**.

That is two independent axes of variation:

```
            block transport            filesystem format
   BIOS  →  ATA PIO  (port I/O)    →    FAT32  (BPB geometry)
   UEFI  →  AHCI/SATA (DMA, MMIO)  →    FAT32  (BPB-derived geometry)
```

If either axis is hardcoded, in-shell file access breaks on the other boot path.
The subsystem therefore inserts **two facades**, each choosing a concrete backend
**once at boot**:

- `drivers::block` — a `BlockDevice` facade over ATA *or* AHCI.
- `io::vfs` — a `FileSystem` facade over FAT32.

Filesystems never call a driver directly; they call `drivers::block`. The syscall
layer and loader never call a filesystem directly; they call `io::vfs`. Once both
globals are selected at boot, the entire upper stack is transport- and
format-agnostic.

---

## 2. Layered Architecture

```
   syscall/dispatch/fs.rs      process/loader.rs        shell `dir`/`cat`/exec
            \                        |                       /
             \                       v                      /
              ------------>   io::vfs   (FileSystem facade) <-----
                                   |    global: MOUNTED_FS
                       +-----------+------------+
                       |                        |
                Fat32Fs (read-only)          (Legacy)
                 io/fat32.rs                
                       |                        |
                       |   io::gpt (ESP discovery, UEFI only)
                       |                        |
                       +-----------+------------+
                                   |
                        drivers::block  (BlockDevice facade)
                                   |    global: ACTIVE_DEVICE
                       +-----------+------------+
                       |                        |
                AtaBlockDevice           AhciBlockDevice
                       |                        |
                drivers::ata             drivers::ahci
              (0x1F0–0x1F7 PIO)        (PCI BAR5 MMIO + DMA)
```

Every sector that moves in KAOS passes through `drivers::block`. Every file
operation passes through `io::vfs`. These two chokepoints are where the boot-path
divergence is resolved.

---

## 3. Layer 1 — Block Transports

Both supported controllers use **512-byte sectors** and are addressed by **LBA**
(Logical Block Address — a flat sector index, sector 0 first). They differ
entirely in *how* a transfer is issued.

### 3.1 ATA PIO Driver (`drivers/ata.rs`)

ATA in **PIO (Programmed I/O)** mode is the simplest possible disk interface: the
CPU programs a handful of I/O ports (the *task file*), issues a command, and then
moves every single 16-bit word of data through a data port itself. No DMA, no
shared memory — just `in`/`out` instructions. It is slow but trivial and always
present on a legacy BIOS machine.

**The task-file registers** (primary bus, base `0x1F0`):

| Port   | Offset | Register | Purpose |
|--------|--------|----------|---------|
| `0x1F0`| 0 | Data (16-bit) | PIO data transfer, one `u16` per access. |
| `0x1F2`| 2 | Sector Count | Number of sectors for this command. |
| `0x1F3`| 3 | LBA low  | LBA bits 0–7. |
| `0x1F4`| 4 | LBA mid  | LBA bits 8–15. |
| `0x1F5`| 5 | LBA high | LBA bits 16–23. |
| `0x1F6`| 6 | Drive/Head | `0xE0` = master + LBA mode, low nibble = LBA bits 24–27. |
| `0x1F7`| 7 | Status / Command | Read = status; write = command byte. |

This is **28-bit LBA addressing** (`lba` is split across four registers, top 4
bits in the drive/head register), so the driver rejects any `lba > 0x0FFF_FFFF`
with `AtaError::LbaOutOfRange`. Commands used: `0x20` READ SECTORS, `0x30` WRITE
SECTORS.

**The status register** drives all synchronization (`StatusRegister`):

- `BSY` (0x80) — controller busy, registers not yet valid.
- `DRQ` (0x08) — Data ReQuest: a sector's worth of data is ready to transfer.
- `DF`  (0x20) — device fault.
- `ERR` (0x01) — error; details in the error register.

A transfer is ready when `!BSY && DRQ`; `ERR`/`DF` abort immediately
(`status_is_ready`).

**A read** (`read_sectors`) proceeds:

1. Lifecycle + geometry validation (`init` must have run; LBA in range; buffer big
   enough — these are `assert!`s, treated as kernel invariants).
2. Acquire the **request slot** (see below) and clear the stale IRQ flag.
3. `setup_command`: wait for `!BSY`, program the task file, write the command byte.
4. For each sector: `wait_ready_or_error()`, then read 256 `u16` words from the
   data port into the buffer in little-endian byte order.

Writes are symmetric, pushing words out to the data port.

#### Cooperative waiting — the interesting part

A naive PIO driver busy-spins on the status port, wasting the CPU for the
milliseconds a disk takes to respond. KAOS instead waits **cooperatively** so
other tasks keep running, using IRQ14 as a wake hint:

- **`primary_ata_irq_handler`** (IRQ14 top-half) does almost nothing: it sets
  `IRQ_EVENT_PENDING` (a `Release` store) and returns. It deliberately performs
  **no** PIO transfer in interrupt context — all data movement stays in
  `read_sectors`/`write_sectors` after the task wakes.
- **`wait_ready_or_error`** samples status; if not ready and a scheduler is
  running with interrupts enabled, it consumes a pending IRQ edge
  (`IRQ_EVENT_PENDING.swap(false)`) or `yield_now()`s and re-checks. This is
  *IRQ-hinted polling*, not pure interrupt-driven I/O: it re-checks status in a
  loop and yields between checks, which is robust against controllers that do not
  deliver a clean IRQ edge for every intermediate state transition. A bounded
  `ATA_POLL_TIMEOUT_ITERATIONS` prevents an infinite hang on dead hardware.
- In early-boot/test contexts (no scheduler yet), it falls back to a plain
  `spin_loop()`.

#### The request slot — serialization without holding a lock across a yield

This is the single most important correctness pattern in the whole subsystem.
ATA can serve only one request at a time, so requests must be serialized. But the
kernel `SpinLock` **disables interrupts** for the lifetime of its guard
(`docs/sync.md`), and an ATA request **yields to the scheduler** while waiting on
the disk. Holding a `SpinLock` across that yield would run another task — or
idle — with interrupts disabled, which is a deadlock/correctness disaster.

The driver resolves this with a two-tier scheme:

- **`REQUEST_IN_FLIGHT`** (`AtomicBool`) + **`REQUEST_WAITQUEUE`**: a request
  acquires exclusive ownership via `acquire_request_slot()`, which either claims
  the slot with a `compare_exchange`, or **sleeps on a wait queue** (cooperative,
  interrupts stay enabled) until the current owner releases it. The
  `RequestSlotGuard` releases the flag and wakes waiters on `Drop`. This guard can
  legally be held across scheduler sleeps — it is *not* a `SpinLock`.
- **`with_controller`** takes the actual `SpinLock<AtaPio>` only for the brief,
  non-yielding bursts that touch the ports (program the task file, or transfer one
  sector's 256 words). It is never held across a wait.

So: long-lived exclusivity via an atomic+waitqueue token; short-lived port
exclusivity via the spinlock. Interrupts are never disabled across a yield.

`primary_present()` is a side-effect-free probe used at boot to tell the two boot
paths apart: it reads the status port (`0x1F7`); a floating, disconnected channel
reads back `0xFF`, anything else means a drive is present. This is how `main.rs`
decides BIOS-vs-UEFI without a dedicated flag, and it works before `init()`.

### 3.2 AHCI / SATA Driver (`drivers/ahci.rs`)

AHCI (Advanced Host Controller Interface) is the modern SATA interface. Unlike
ATA PIO, the CPU does **not** move data — it builds command structures in RAM,
points the controller at them via MMIO registers, and the controller **DMAs** the
data to/from a buffer. This is the only disk interface available on a UEFI machine
with no legacy IDE.

#### Discovery and MMIO

AHCI controllers are PCI devices of **class `0x01` (mass storage), subclass `0x06`
(SATA)**. `init()` scans *every* such controller (`pci::get_devices()`) and uses
the first one with an active SATA port. **Multiple controllers are common**: on
QEMU/Proxmox q35 the built-in ICH9 AHCI at `00:1f.2` is present but empty while
the disk hangs off a *separate* `ich9-ahci` controller — picking the first blindly
talks to the wrong HBA (the `det=0 on all ports` failure). Hence the full scan.

For each controller, `try_init_controller`:

1. Enables **Memory Space** (bit 1) and **Bus Master** (bit 2) in the PCI command
   register — real firmware may leave these off, which would make MMIO and DMA
   silently fail.
2. Reads **BAR5** = the **ABAR** (AHCI Base Address Register), the physical base of
   the HBA's memory-mapped registers.
3. Identity-maps the ABAR pages (the 32-port register block fits in two 4 KiB
   pages) via the VMM.
4. Sets `GHC.AE` (bit 31) to enable AHCI mode, then brings up ports.

The register layout is described by `#[repr(C)]` structs mirroring the AHCI spec:
`HbaMem` (global host control: `cap`, `ghc`, `pi` ports-implemented bitmap, …)
containing `ports: [HbaPort; 32]`. Each `HbaPort` has `clb`/`clbu` (command list
base), `fb`/`fbu` (received-FIS base), `cmd`, `tfd` (task file), `sig`
(device signature), `ssts` (SATA status), `sctl` (SATA control), `ci` (command
issue), etc. All accesses use `read_volatile`/`write_volatile` because these are
MMIO registers.

#### Per-port bring-up

`init_ports` walks the `pi` bitmap and, for each implemented port:

- **Fast-skip empty ports**: if `PxSSTS.DET == 0` (no device detected) and the
  controller lacks staggered spin-up (`CAP.SSS`), the port is empty — skip it
  before paying any link-training cost. Without this, the empty built-in HBA costs
  seconds per port.
- **`port_rebase`**: stop the command engine (clear `ST`/`FRE`, wait for `CR`/`FR`
  to clear), allocate **one physical 4 KiB frame** from the PMM, and carve it into
  the AHCI port structures:

  ```
  frame + 0      Command List    (1024 bytes; slot 0's HbaCmdHeader lives here)
  frame + 1024   Received FIS    (256 bytes)
  frame + 1280   Command Table   (HbaCmdTbl: command FIS + PRDT)
  frame + 1536   DMA data buffer (512 bytes — one sector; DMA_BUFFER_PHYS)
  ```

  It links the command table into command-list slot 0, then enables **FIS
  reception** (`FRE`) so the device's signature FIS is latched.
- **`port_bring_up`** (only if the link is not already `DET == 3`): clears latched
  SATA errors, optionally requests spin-up / power-on, forces the interface to the
  active power state, then issues a **COMRESET** (drive `PxSCTL.DET = 1` for ~2 ms,
  then back to 0). COMRESET is the *only* way to pull a Phy out of the offline
  (`DET == 4`) state that real firmware often hands over. It then waits for the
  link to establish (`DET == 3`), with a short *presence* budget (50 ms) for ports
  that never report a device and a longer *link-training* budget (200 ms) once a
  device is detected.
- **`port_wait_ready`** waits for `PxTFD` to clear `BSY|DRQ`, then the signature
  `PxSIG` is checked against `SATA_SIG_ATA` (`0x00000101`) to confirm an ATA disk.
- **`port_start`** sets `ST` (+`FRE`) to start the command engine, and the port is
  recorded in the global `ACTIVE_PORT`.

> **KVM caveat — `delay_ms`.** No timer is wired up this early on the UEFI path, so
> bring-up delays use a calibrated arithmetic loop. It deliberately does **not**
> use `spin_loop()` (PAUSE): under KVM/Proxmox, PAUSE-loop-exiting turns a tight
> PAUSE loop into a storm of VM exits that made `init` appear to hang for minutes.
> A plain arithmetic loop with a `read_volatile` sink runs at native speed inside
> the guest and triggers no VM exits.

#### Issuing a read — `read_sectors`

AHCI reads are read-only in this iteration and transfer **one sector per command**
into the fixed DMA buffer, then copy it into the caller's buffer:

1. Wait for command slot 0 to be free (`PxCI` and `PxSACT` bit clear).
2. Fill in the **command header** (`HbaCmdHeader`): command-FIS length in DWORDs,
   `prdtl = 1` (one PRDT entry), read direction.
3. Fill in the **PRDT entry** (`HbaPrdtEntry`): `dba`/`dbau` = physical address of
   the 512-byte DMA buffer, `dbc = 511` (byte count is 0-indexed).
4. Fill in the **command FIS** (`FisRegH2D`, a Host-to-Device Register FIS, type
   `0x27`): command `0x25` = **READ DMA EXT**, the 48-bit LBA split across
   `lba0..lba5`, `device = 1<<6` (LBA mode), count = 1 sector.
5. Issue it by writing `PxCI = 1 << slot`. Poll `PxCI` for completion; abort on a
   Task File Error (`PxIS` bit 30) or timeout.
6. `copy_nonoverlapping` the DMA buffer into the caller's buffer at the right
   offset.

`AhciError { NotInitialized, PortError, Timeout }`. There is **no
`write_sectors`** — AHCI writes are deliberately out of scope (see §3.3 and §8).

### 3.3 The `BlockDevice` Facade (`drivers/block.rs`)

This is the abstraction that erases the ATA-vs-AHCI difference. It is a small,
self-contained module:

```rust
pub const SECTOR_SIZE: usize = 512;

pub trait BlockDevice: Send + Sync {
    fn read_sectors(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_sectors(&self, lba: u64, count: u32, buf: &[u8]) -> Result<(), BlockError>;
    fn sector_size(&self) -> usize { SECTOR_SIZE }
}
```

Note the **argument order `(lba, count, buf)`** — different from the underlying
drivers' `(buf, lba, count)`. The trait uses `u64`/`u32` for `lba`/`count` so it
outlives 28-bit ATA; the adapters clamp to their hardware's real limits.

Two zero-sized adapters implement it:

- **`AtaBlockDevice`** — read + write; clamps the LBA ceiling to `0x0FFF_FFFF`
  (28-bit ATA) and maps `AtaError → BlockError::Device`.
- **`AhciBlockDevice`** — read forwards to `ahci::read_sectors`; **write returns
  `BlockError::Unsupported`** (AHCI is read-only). LBA ceiling is `u32::MAX` until
  48-bit LBA is wired through.

Both go through the **`chunked`** helper, which:

- returns early on `count == 0`;
- rejects out-of-range/overflowing `(lba, count)` against the device ceiling
  (`BlockError::OutOfRange`);
- splits the request into **≤255-sector** hardware commands
  (`MAX_SECTORS_PER_CMD`, since the hardware sector count is a `u8`), invoking the
  per-command closure with a running byte offset into the caller's buffer.

`check_buf` rejects an undersized buffer (`BlockError::BadBuffer`).

**Selection and the lock discipline:**

```rust
static ATA_DEVICE: AtaBlockDevice = AtaBlockDevice;
static AHCI_DEVICE: AhciBlockDevice = AhciBlockDevice;
static ACTIVE_DEVICE: SpinLock<Option<&'static dyn BlockDevice>> = SpinLock::new(None);

pub fn init_ata()  { *ACTIVE_DEVICE.lock() = Some(&ATA_DEVICE); }
pub fn init_ahci() { *ACTIVE_DEVICE.lock() = Some(&AHCI_DEVICE); }
```

The active device is a `&'static dyn BlockDevice` (a fat pointer) behind a
`SpinLock<Option<…>>`. Storing a `'static` reference avoids `AtomicPtr`
fat-pointer problems and needs no `unsafe`. Crucially, the public
`read_sectors`/`write_sectors` **lock only to copy the reference out, then release
the lock before calling the driver**:

```rust
pub fn read_sectors(lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
    let dev = { let g = ACTIVE_DEVICE.lock(); (*g).ok_or(BlockError::NotReady)? };
    dev.read_sectors(lba, count, buf)   // lock already released — ATA may yield here
}
```

This mirrors the request-slot rule in the ATA driver: **never hold a `SpinLock`
across a disk call**, because the ATA path yields. `reset_active_device()` exists
for test isolation.

---

## 4. Layer 2 — Partition Discovery: GPT (`io/gpt.rs`)

Only the UEFI path needs this. A GPT disk has a header at **LBA 1** (signature
`"EFI PART"`) describing a partition-entry array. `find_esp_start_lba`:

1. Reads LBA 1 via `block::read_sectors`, parses the header
   (`parse_gpt_header`) to get `(entry_lba, num_entries, entry_size)` from offsets
   `0x48`/`0x50`/`0x54`.
2. Iterates the partition-entry sectors, comparing each entry's 16-byte **type
   GUID** against the EFI System Partition GUID (`ESP_TYPE_GUID`, stored in the
   on-disk mixed-endian byte order).
3. On a match, returns the partition's starting LBA (entry offset `0x20`), which
   becomes the FAT32 mount's `part_lba`.

If parsing fails or no ESP is found, `fallback_esp()` returns **LBA 2048** (the
conventional first-partition offset) — a pragmatic fallback flagged with a TODO to
remove once GPT parsing is fully trusted.

---

## 5. Layer 3 — Filesystems

Both filesystems are members of the **FAT** family. Shared concepts:

- A file's data lives in a chain of **clusters** (one or more sectors each). The
  **File Allocation Table (FAT)** is an on-disk array: `FAT[n]` gives the *next*
  cluster after cluster `n`, or an end-of-chain marker. Following the chain reads
  the file.
- A **directory** is a list of fixed **32-byte entries**: an 8.3 name (8 base + 3
  extension, space-padded, uppercase), an attribute byte (bit 4 = directory, `0x0F`
  = long-file-name helper to skip, `0x08` = volume label), the first cluster, and
  the file size. First name byte `0x00` = end of directory, `0xE5` = deleted slot.

The two differ in geometry and FAT-entry width.

### 5.1 FAT32 (`io/fat32.rs`, the UEFI path — read-only)

FAT32 is a real, parsed filesystem: geometry comes from the **BIOS Parameter
Block (BPB)** in the partition's first sector, not from constants.
`Fat32Volume::mount(part_lba)`:

1. Reads the BPB via `block::read_sectors`.
2. Validates FAT32: bytes/sector must be 512; `RootEntCnt` and `FATSz16` must be 0
   (those are FAT12/FAT16 specific fields); boot signature `0xAA55` present.
3. Computes the key LBAs:
   ```
   fat_start_lba  = part_lba + reserved_sectors
   data_start_lba = fat_start_lba + num_fats * fat_size_32
   cluster_to_lba(n) = data_start_lba + (n - 2) * sectors_per_cluster
   ```
   and remembers `root_cluster` (FAT32's root directory is itself a cluster chain,
   not a fixed region).

**`next_cluster`** reads the 4-byte FAT32 entry (`fat_start_lba + cluster*4/512`,
offset `cluster*4 % 512`), masks the top 4 reserved bits (`& 0x0FFF_FFFF`), and
classifies: `0x0FFF_FFF7` = bad, `>= 0x0FFF_FFF8` = end-of-chain, `< 2` = corrupt.

**`read_file`** walks the root-directory cluster chain looking for the normalized
8.3 name (`normalize_name`), reading 16 directory entries per 512-byte sector,
skipping deleted (`0xE5`) and LFN (`attr == 0x0F`) entries, rejecting directories
(`attr & 0x10`). The 32-bit first cluster is reassembled from its high word
(offset `0x14`) and low word (offset `0x1A`). It then walks the file's chain into
a `Vec<u8>`, copying only `file_size` bytes (trimming last-sector padding), capped
at 8 MiB and guarded against chain loops by a `cluster_count` ceiling.
`print_root_directory` is the `dir` listing equivalent.

The current implementation reads **one sector per `block::read_sectors` call**
inside the cluster loops — correct but not maximally efficient; the chunking in
`drivers::block` would allow whole-cluster reads as a future optimization.

---

## 6. Layer 4 — The VFS Facade (`io/vfs.rs`)

The VFS erases the underlying filesystem details exactly as `drivers::block` erases
the controller difference.

```rust
pub trait FileSystem: Send + Sync {
    fn open(&self, name: &str, mode: FileMode) -> Result<usize, FsError>;
    fn close(&self, fd: usize) -> Result<(), FsError>;
    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize, FsError>;
    fn seek(&self, fd: usize, offset: u32) -> Result<(), FsError>;
    fn eof(&self, fd: usize) -> Result<bool, FsError>;
    fn delete(&self, name: &str) -> Result<(), FsError>;
    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError>;   // whole-file, for the loader
    fn print_root_directory(&self);
}
```

A **single global mounted filesystem** is held in
`MOUNTED_FS: SpinLock<Option<Box<dyn FileSystem>>>` (a `Box`, because the FS object
is built at runtime and lives on the heap — by mount time the heap exists).
`mount()` publishes it once at boot. There is no mount table; one filesystem is
enough for KAOS.

#### The `with()` helper — lock discipline again

The same "no `SpinLock` across disk I/O" hazard from §3.1/§3.3 applies here, and
the solution is the same shape:

```rust
fn with<R>(f: impl FnOnce(&dyn FileSystem) -> Result<R, FsError>) -> Result<R, FsError> {
    let ptr: *const dyn FileSystem = {
        let guard = MOUNTED_FS.lock();
        match guard.as_deref() {
            Some(fs) => fs as *const dyn FileSystem,
            None => return Err(FsError::NotMounted),
        }
    }; // lock released here
    // SAFETY: MOUNTED_FS is mount-once at boot, never replaced/freed; the Box
    // outlives the kernel. FS impls use interior mutability (their own SpinLock'd
    // FD tables) so &self is sound and there is no &mut aliasing.
    f(unsafe { &*ptr })
}
```

It copies the trait-object fat pointer out under the lock, releases the lock, then
calls the FS through the raw pointer. The `unsafe` is justified because the mount
is set once and never dropped, and every backend uses interior mutability (so all
trait methods take `&self`). All the public facade functions
(`open`/`read`/`write`/…) are one-liners over `with(...)`.

#### The two adapters

  thin wrapper over the already-working FD layer, so the BIOS path keeps its full
  read/write/delete behaviour. The error map distinguishes `NotFound` as
  `FsError::NotFound` (file missing) vs `FsError::InvalidFd` (bad FD) using an
  `fd_context` flag.
- **`Fat32Fs`** (`io/fat32.rs`) — wraps a `Fat32Volume` plus a small
  `SpinLock<Vec<Option<Fat32OpenFile>>>` FD table. Because `Fat32Volume` has no FD
  layer of its own, `open()` uses the **eager-cache** strategy: it calls
  `volume.read_file(name)` once (doing all disk I/O *before* taking the FD-table
  lock, honouring the lock discipline), caches the whole `Vec<u8>`, and then
  `read`/`seek`/`eof` operate purely on that in-RAM buffer (no further disk I/O,
  so locking is fine). All write-side methods (`write`, `delete`) return
  `FsError::Unsupported`, so a write attempt on a UEFI boot fails **cleanly**
  rather than panicking or corrupting.

---

## 7. Consumers — Syscalls, Loader, Boot Wiring

### 7.1 Filesystem syscalls (`syscall/dispatch/fs.rs`)

The user-space shell's file operations are syscalls that funnel into `io::vfs`:
`syscall_open_file_impl` → `vfs::open`, and likewise close/read/write/seek/eof/
delete/print-root-directory. User string pointers are validated with
`read_user_string` (bounded, `is_valid_user_buffer`-checked, UTF-8 validated). The
mode integer maps to `vfs::FileMode`. `FsError` is translated to the syscall ABI's
`SyscallError` via `map_fs_error`, so `FsError::Unsupported` from a FAT32 write
surfaces to the shell as a clean error.

### 7.2 Program loader (`process/loader.rs`)

Loading an executable (`SHELL.BIN`, or `exec` of another program) calls
`vfs::read_file(name)` for the whole image and maps `FsError → ExecError`
(`map_fs_error`). Because this goes through the VFS, "exec a program from the
shell" works identically on both boot paths.

### 7.3 Boot wiring (`main.rs`)

`main.rs` picks the boot path with
`let uefi = booted_via_framebuffer(...) && !drivers::ata::primary_present();`
and then assembles the right stack — *driver → block device → filesystem* — in
strict order:

```rust
let shell_image = if uefi {
    drivers::ahci::init();                 // bring up the AHCI controller
    drivers::block::init_ahci();           // select AHCI as the active block device
    let esp_lba = io::gpt::find_esp_start_lba().expect("ESP not found");
    let vol = io::fat32::Fat32Volume::mount(esp_lba).expect("FAT32 mount failed");
    io::vfs::mount(Box::new(io::fat32::Fat32Fs::new(vol)));   // mount FAT32
    io::vfs::read_file("shell.bin").expect("read SHELL.BIN from ESP")
} else {
    drivers::ata::init();                  // bring up ATA PIO
    drivers::block::init_ata();            // select ATA as the active block device
        io::vfs::mount(Box::new(io::fat32::Fat32Fs::new(volume)));             // mount FAT32
    io::vfs::read_file("shell.bin").expect("load SHELL.BIN from FAT32")
};
```

**Ordering matters**: `block::init_ahci()` must precede `gpt::find_esp_start_lba()`
and the FAT32 mount, because both read sectors through `drivers::block` and would
get `BlockError::NotReady` if no device is selected yet. After this, both paths
have a selected block device and a mounted filesystem, and everything above —
syscalls, loader, `dir`, `cat` — is identical.

---

## 8. Concurrency Model (the recurring theme)

The whole subsystem is shaped by one hard rule:

> **Never hold an interrupt-disabling `SpinLock` across a disk operation, because
> the ATA path yields to the scheduler while waiting on IRQ14.**

It shows up three times, each solved the same way — *acquire the lock only long
enough to copy out a stable reference / token, then release it before the
potentially-yielding call*:

| Layer | Long-lived exclusivity | Short-lived lock |
|-------|------------------------|------------------|
| ATA driver | `REQUEST_IN_FLIGHT` atomic + `REQUEST_WAITQUEUE` (`RequestSlotGuard`, held across yields) | `SpinLock<AtaPio>` via `with_controller`, only for port bursts |
| `drivers::block` | — | `ACTIVE_DEVICE` lock copied out before the driver call |
| `io::vfs` | — | `MOUNTED_FS` lock copied out (`with()`) before the FS call |

Within a filesystem, the FD tables (`FILE_DESCRIPTORS`, `Fat32Fs::open_files`) are
`SpinLock`-guarded, but they are only ever held while touching RAM, never across a
sector read — `Fat32Fs::open` carefully does its `read_file` *before* taking the
FD-table lock.

---

## 9. Error Model

Errors are translated layer by layer, each enum narrowing to its caller's
vocabulary. Every conversion is total (no panics on the error path):

```
AtaError / AhciError
        │  (adapter .map_err)
        ▼
BlockError {NotReady, BadBuffer, OutOfRange, Device, Unsupported}
        │  (Fat32Error::Block(..))
        ▼
Fat32Error
        │  (Fat32Fs map_fat32_err)
        ▼
FsError {NotMounted, NotFound, InvalidFd, Unsupported, Io, InvalidName}
        │  (syscall map_fs_error / loader map_fs_error)
        ▼
SyscallError   /   ExecError
```

The `Unsupported` variant is threaded all the way through specifically so a
write/delete on the read-only FAT32 mount degrades to a clean shell-level error
instead of a panic or silent corruption.

---

## 10. Current Limitations & Future Work

These are deliberate scope boundaries (see `docs/storage_abstraction.md` §10):

- **AHCI is read-only.** `AhciBlockDevice::write_sectors` returns `Unsupported`;
  FAT32 write methods do too. The filesystem is currently read-only.
- **AHCI transfers one sector per command** and uses a single fixed 512-byte DMA
  buffer and command slot 0. Multi-sector PRDTs / multiple slots are future work.
- **AHCI uses 28→32-bit LBA in practice**; the block ceiling is `u32::MAX` until
  48-bit LBA is plumbed through, even though READ DMA EXT itself is 48-bit capable.
- **AHCI bring-up is polled** with calibrated delays (no interrupt-driven
  completion, no real timer at init).
- **Single mount, no mount table.** One filesystem at a time.
- **FAT32 reads sector-by-sector**; whole-cluster reads are an available
  optimization now that `drivers::block` chunks.
  from its boot sector.

The trait shapes (`BlockDevice`, `FileSystem`) were chosen to leave room for all of
the above without disturbing the upper layers.

---

## 11. File Reference

| File | Responsibility |
|------|----------------|
| `drivers/ata.rs` | ATA PIO driver: task-file ports, 28-bit LBA, IRQ14-hinted cooperative wait, request slot. |
| `drivers/ahci.rs` | AHCI/SATA driver: PCI discovery, MMIO HBA/port registers, command list/FIS/PRDT, COMRESET bring-up, DMA reads. |
| `drivers/block.rs` | `BlockDevice` trait, ATA/AHCI adapters, `chunked`, `ACTIVE_DEVICE` selection + facade. |
| `io/gpt.rs` | Minimal GPT parsing to locate the EFI System Partition (UEFI path). |
| `io/fat32.rs` | `Fat32Volume` (BPB parse, chain walk, read-only) + `Fat32Fs` eager-cache VFS adapter. |
| `io/vfs.rs` | `FileSystem` trait, `FsError`, `MOUNTED_FS`, `with()` facade. |
| `syscall/dispatch/fs.rs` | Filesystem syscalls → `io::vfs`. |
| `process/loader.rs` | Whole-image load via `vfs::read_file`. |
| `main.rs` | Boot-path detection + driver/block/FS assembly order. |

---

## See Also

- `docs/storage_abstraction.md` — the design plan and phase-by-phase migration that
  produced this architecture (block + VFS facades).
- `docs/pci.md` — PCI enumeration that AHCI discovery builds on.
- `docs/sync.md` — `SpinLock` and `WaitQueue` semantics central to the lock
  discipline.
- `docs/scheduling.md` — the cooperative `yield_now`/wait-queue mechanics the ATA
  driver relies on.
- `docs/boot_bios.md` / `docs/boot_uefi.md` — how each boot path reaches `main.rs`
  with its respective controller and filesystem.
- `docs/pmm.md` / `docs/vmm.md` — frame allocation and identity-mapping used by
  AHCI for its DMA structures and ABAR registers.
