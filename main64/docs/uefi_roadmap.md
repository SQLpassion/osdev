# UEFI Migration of KAOS — Concept & Roadmap

This document summarizes the analysis and the decision to migrate KAOS from the legacy BIOS boot
to **UEFI**. It describes the *why*, the central technical relationships, and the prioritized
implementation roadmap.

> **Project goals that everything aligns to:**
> 1. **Learn kernel internals**
> 2. **Be runnable on real hardware**
>
> Together, both goals justify the UEFI migration — but deliberately late and in small,
> always-runnable steps, not as a big bang.

---

## 1. Starting Point (current state)

| Area | Current state | Relevant files |
|---|---|---|
| **Boot** | 3-stage legacy BIOS boot (real mode → long mode) | `boot/bootsector.asm`, `kaosldr_16/`, `kaosldr_64/` |
| **Disk (runtime)** | ATA PIO via legacy ports `0x1F0`–`0x1F7` | `kernel/src/drivers/ata.rs` |
| **Filesystem** | FAT12, 1.44 MB floppy geometry, no VFS | `kernel/src/io/fat12/` |
| **Console** | VGA text mode, memory-mapped `0xB8000` (80×25) | console layer / `print_root_directory` |
| **Image** | FAT12 floppy via `fat_imgen`, no GPT/no partition | `build.sh` |
| **PCI** | Full enumeration incl. BAR parsing present | `kernel/src/drivers/pci/` |

Every boot stage is **fully BIOS-dependent** (real-mode assembly, INT 0x10/0x13/0x15/0x1A,
A20 gate, hand-built page tables) and will not start on modern UEFI-only hardware.

---

## 2. Two Central Insights (why it is not a chicken-and-egg problem)

### 2.1 Boot-time disk access ≠ runtime disk access

```
┌─ BOOT PHASE ──────────────────┐     ┌─ RUNTIME (kernel running) ────┐
│  Disk access via FIRMWARE      │     │  Disk access via OWN           │
│  (BIOS INT 0x13 / UEFI BlockIo)│ ──▶ │  kernel driver (ATA/AHCI/NVMe) │
│  loads only: KERNEL.BIN (+img) │     │  everything after: read_file() │
└────────────────────────────────┘     └────────────────────────────────┘
```

**The kernel never has to load itself from disk.** The firmware (BIOS *or* UEFI) brings its own
disk driver for that. UEFI has ready-made drivers for **AHCI and NVMe**. Only *after*
`ExitBootServices()` — when the kernel already reigns in RAM — does the kernel need its own
driver. This dissolves the apparent chicken-and-egg problem.

### 2.2 ATA PIO is a question of controller mode, not of UEFI

A SATA controller can operate in two modes:

| Mode | Addressing | ATA PIO (`0x1F0`…)? |
|---|---|---|
| **IDE / Legacy / Compatibility** | Legacy ports `0x1F0`–`0x1F7` | ✅ — today's `ata.rs` driver |
| **AHCI** | MMIO via the PCI BAR5 | ❌ legacy ports dead |

Modern UEFI-only boards practically always configure the controller in **AHCI mode**
(IDE compatibility is tied to CSM/legacy and often no longer selectable).
→ On such hardware, the legacy ATA PIO driver **no longer works at runtime**.
**NVMe** never had ATA ports anyway (a completely different PCIe interface) and needs its own
driver.

**Consequence:** As soon as the kernel needs native runtime access to SATA/NVMe, it requires an
AHCI or NVMe driver. *For the boot itself* it never does (the firmware handles it).

---

## 3. What UEFI Takes Away From the Kernel: the VGA text mode

The text buffer at `0xB8000` (word format `[character][attribute]`) is **not memory, but a
hardware feature of the VGA card in text mode**. The **BIOS** sets up this mode at startup —
the kernel today merely benefits from it.

**UEFI does not do this.** UEFI initializes graphics via the **GOP (Graphics Output Protocol)**
and puts the card into a **graphics mode with a linear framebuffer**. In this state:

- writing to `0xB8000` produces **nothing** on screen,
- on many real UEFI machines the legacy VGA hardware is no longer wired up at all.

Text output under UEFI, clearly separated by phase:

| Phase | Text output |
|---|---|
| **During Boot Services** (before `ExitBootServices`) | UEFI's own console via the `ConOut` / `SimpleTextOutput` protocol (only via UEFI call, **not** memory-mapped) — convenient for first tests |
| **After `ExitBootServices`** (kernel reigns) | **only the GOP framebuffer** — text must be rendered manually: font glyphs → pixels, scrolling, cursor |

→ The entire VGA text-mode console layer must be converted to **pixel rendering**.
This is the actual effort driver of the UEFI migration (hence pulled forward in the roadmap).

---

## 4. Image Format: from Floppy to GPT/ESP

Today's `fat_imgen` floppy image (1.44 MB, no GPT, no ESP) is **not** UEFI-bootable.
Instead:

- **GPT partition table** with one **EFI System Partition (ESP)**, FAT-formatted.
- Inside it, the path **`/EFI/BOOT/BOOTX64.EFI`** — the loader searched by convention
  automatically. That is where the UEFI loader binary goes (PE32+ from `x86_64-unknown-uefi`).

Image build (instead of `fat_imgen`, most robust in the existing Docker build env):
`dd`/`qemu-img` → GPT (`sgdisk`/`parted`, ESP type `EF00`) → `mkfs.fat` → `mtools` (`mcopy`)
for `BOOTX64.EFI`, `KERNEL.BIN`, etc.

The same GPT/ESP image boots **identically in QEMU and on real hardware**.

---

## 5. Roadmap (prioritized)

> Guiding principle: every step is self-contained, low-risk, and keeps the system runnable at
> all times. The most expensive chunk (framebuffer) is pulled forward and done while the BIOS
> boot still works.

### Step 1 — GOP framebuffer console in the kernel *(first!)*
Convert the new console layer to pixel rendering (font, scrolling, cursor).
**Still testable under BIOS**: via VBE/VESA a linear framebuffer can also be obtained in the
BIOS boot. This decouples "learning the framebuffer" from "debugging the UEFI boot without GDB".
High learning value, no boot risk.

### Step 2 — UEFI loader
Risk is now small: the console already runs via the framebuffer. Pure boot conversion:
- PE32+ EFI application (`x86_64-unknown-uefi`),
- use Boot Services: get memory map → get GOP framebuffer info → load `KERNEL.BIN`
  → `ExitBootServices()` → jump to the kernel,
- GPT/ESP image (see §4).

The kernel from physical `0x100000` remains **boot-agnostic** as long as the handover convention
(memory map + framebuffer info) is satisfied. **Keep BIOS and UEFI boot in parallel for a while**
to enable isolated debugging.

### Step 3 — On real hardware
- Image via `dd` onto a USB stick (`sudo dd if=… of=/dev/rdiskN bs=1m`),
- in the UEFI setup **disable Secure Boot** (binary is unsigned), disable CSM/legacy,
- the firmware finds `/EFI/BOOT/BOOTX64.EFI` automatically,
- **serial logging as the debug channel** (no GDB on real HW) — the test runner already uses
  `-serial stdio`; the same logic carries over to real hardware.

### Step 4 — Real AHCI driver (native SATA access)
Based on the existing PCI enumeration: map BAR5 MMIO, HBA reset, command list + FIS,
DMA buffers, IRQ completion (the ATA IRQ wait infrastructure serves as a template). This
provides native runtime disk access on real UEFI hardware for the first time. NVMe optional
afterward.

> **Prerequisite for clean integration:** a `BlockDevice` trait (FAT12 today calls
> `drivers::ata::*` directly). ATA PIO, RAM disk, AHCI, and NVMe then become interchangeable
> implementations of the same trait; FAT12/VFS is unaware of the underlying device.
> ATA PIO can be **kept** for QEMU tests.

---

## 6. Parallel Track: Proof of Concept

Goal: verify **early and independently** that the toolchain and the UEFI boot path work —
before touching the actual kernel.

**Milestone 0 — "does UEFI run at all?"**
Minimal `BOOTX64.EFI` that only prints a string via `ConOut`. No kernel, no disk,
no framebuffer. Proves: the toolchain (`x86_64-unknown-uefi`) builds a runnable PE32+,
the GPT/ESP layout is correct, QEMU+OVMF (or real firmware) finds and starts the binary.

**Milestone 1 — "Rust kernel into RAM + GOP output"**
The loader obtains the GOP via Boot Services, loads a simple Rust kernel binary from the ESP into
RAM, calls `ExitBootServices()`, jumps — the kernel outputs something on screen via the **GOP
framebuffer** (e.g. a color gradient or a few rendered glyphs).

This proves the entire UEFI path in isolation and can then be merged with the "real" migration
(steps 1–4).

### QEMU with UEFI (OVMF)

QEMU does not emulate UEFI out of the box — you provide it the free EDK2 firmware **OVMF**
(the same one that brings AHCI/NVMe drivers as firmware):

```bash
# OVMF ships e.g. with Homebrew QEMU:
qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file=OVMF_CODE.fd \
    -drive if=pflash,format=raw,file=OVMF_VARS.fd \
    -drive format=raw,file=kaos64-uefi.img \
    -m 256M
```

This later also allows attaching native SATA/NVMe devices for testing:

```bash
    -drive format=raw,file=disk.img,if=none,id=nvm \
    -device nvme,serial=deadbeef,drive=nvm        # NVMe driver test
```

---

## 7. Optional Later Step: RAM Disk (runtime FS without a driver)

If the kernel needs the filesystem at runtime (KAOS loads user programs on-demand via
`fat12::read_file()` in `process/loader.rs`) but **no** AHCI/NVMe driver exists yet:

> The UEFI loader reads the **entire** FAT12 image into RAM using the firmware drivers.
> FAT12/VFS then reads from RAM instead of from disk — as another `BlockDevice` implementation
> (`RamDisk`).

This makes the existing program loading + file I/O work **without an own disk driver** on real
hardware. Limitation: writes are volatile (do not survive a reboot). Persistence only comes with
the real driver (step 4). For the **pure UEFI boot test** (PoC) this is not yet needed.

---

## 8. Secure Boot (very end, optional)

Secure Boot checks exactly one thing: that the loaded `.EFI` binary is signed with a key from the
firmware `db`. By default only Microsoft's key is in there.

- **Linux** solves this via **shim** (Microsoft-signed) + **MOK** (Machine Owner Key) to boot on
  foreign PCs without enrolling keys. Plus a "lockdown" mode in the kernel that closes all
  post-boot ring-0 write paths (unsigned modules, `/dev/mem`, `kexec`) (`CONFIG_MODULE_SIG`).
  A chain of trust is only as strong as its weakest post-boot write access to ring 0.
- **For KAOS** on its own hardware, the simpler path suffices: **enroll your own keys**
  (generate PK/KEK/db via `openssl`/`sbsign`, enroll in "Custom Mode" in the UEFI setup,
  sign the binary). No shim/MOK needed.
- **Important:** Secure Boot only checks the loader. A *continuous* chain requires the loader
  itself to verify `KERNEL.BIN` (and the dynamically loadable drivers, see driver
  infrastructure) before the jump — otherwise the chain breaks at the first `read_file()`. For a
  learning OS usually more effort than benefit → deliberately at the end of the roadmap.

---

## 9. Summary

- UEFI is worthwhile for KAOS because "real, modern hardware" is a goal (legacy/CSM is dying out)
  and the seemingly most expensive part (framebuffer console) is **itself kernel-internals
  learning material**.
- No chicken-and-egg: boot-time disk access is done by the firmware, runtime disk access by the
  own driver.
- Order: **(1) GOP console → (2) UEFI loader → (3) real hardware → (4) AHCI driver**,
  in parallel a **PoC** (milestones 0/1) for early toolchain validation.
- AHCI/NVMe are pure runtime topics and do not block the UEFI boot test; a RAM disk bridges the
  runtime FS gap until the real driver is ready.
- Secure Boot is additive and comes last.
