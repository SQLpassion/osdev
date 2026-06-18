# The KAOS UEFI Loader — Build & Boot Pipeline, and the PE/COFF Format

This document explains, in detail, **how the `kaosldr_uefi` loader is built and booted** and
**why a UEFI executable is a Microsoft PE/COFF file** even though UEFI is an open standard.

For the strategic *why/when* of the UEFI migration and the overall roadmap, see
[`uefi_roadmap.md`](uefi_roadmap.md). This document is the concrete, mechanical companion to it.

---

## 1. What `cargo build` produces

Building the loader crate:

```
cd kaosldr_uefi && cargo build
```

produces a single artifact:

```
kaosldr_uefi/target/x86_64-unknown-uefi/debug/bootx64.efi
```

`file` reports it as: **`PE32+ executable (EFI application) x86-64`**. Three properties are
fixed by the `x86_64-unknown-uefi` target:

1. **File format = PE/COFF**, not ELF. PE32+ ("Portable Executable", 64-bit variant) is the
   executable format UEFI mandates. The `.efi` extension is applied automatically for this
   target; the base name `bootx64` comes from the `[[bin]]` entry in `Cargo.toml`.
2. **PE subsystem = "EFI Application".** A field in the PE header tells the firmware this image
   is a UEFI application (as opposed to an EFI boot-service or runtime driver).
3. **Entry point = `efi_main`.** The linker (`rust-lld`) wires the entry to the `efi_main`
   symbol in `src/main.rs`, declared with the `extern "efiapi"` calling convention the firmware
   expects (see §5).

Build details specific to this project:

- There is **no prebuilt `core` guaranteed** for `x86_64-unknown-uefi` in every environment, so
  the toolchain provides `core`/`compiler_builtins` for the target (installed in the dev
  container image, see `.devcontainer/Dockerfile`). The `rust-src` component backs this.
- **No external crates.** The minimal UEFI structures (System Table, Simple Text Output
  Protocol) and the COM1 serial driver are hand-declared in `src/main.rs` and `src/serial.rs`.

**What `bootx64.efi` is *not*:** it is not a disk image, not a partition, and not a boot sector.
It is just the bare executable. Turning it into something the firmware can find and start is the
job of `build_uefi.sh` (§3–§4).

---

## 2. The build & staging pipeline

```
  kaosldr_uefi/src/{main.rs, serial.rs}
            │
            │  cargo build   (target = x86_64-unknown-uefi, linker = rust-lld)
            ▼
  target/x86_64-unknown-uefi/debug/bootx64.efi      ← PE32+ "EFI Application"
            │
            │  cp                                    (build_uefi.sh, step 2)
            ▼
  kaosldr_uefi/esp/EFI/BOOT/BOOTX64.EFI             ← UEFI fallback boot path
            │
            │  staged alongside the firmware files:
            ├─ OVMF_CODE.fd   (firmware code, read-only)        ┐
            ├─ ovmf_vars.fd   (writable NVMR copy, step 3)      ├─ handed to QEMU (step 5)
            └─ esp/           (exposed as a FAT disk via VVFAT) ┘
            ▼
  qemu-system-x86_64  -drive pflash(CODE) -drive pflash(VARS) -drive fat:rw:esp ...
```

### Step 1 — Build
`cargo build` in the crate directory → `bootx64.efi` (§1).

### Step 2 — Stage the ESP layout
```
kaosldr_uefi/esp/EFI/BOOT/BOOTX64.EFI   ← copy of bootx64.efi
```
This encodes the **UEFI boot convention**: with no extra configuration, a UEFI firmware searches
a FAT-formatted EFI System Partition (ESP) for the fixed *fallback / removable-media* path
**`/EFI/BOOT/BOOTX64.EFI`** (the x86-64 default). The script reproduces exactly this directory
tree in a host folder (`esp/`) and drops the loader there as `BOOTX64.EFI`. Nothing else is
required for the firmware to start it.

### Step 3 — Obtain the OVMF firmware
QEMU does **not** emulate UEFI itself — it needs a firmware image. That is **OVMF** (the EDK2
build of UEFI), in two files:

| File | Role | Access |
|---|---|---|
| `OVMF_CODE` (`edk2-x86_64-code.fd` / `OVMF_CODE_4M.fd`) | the firmware code | **read-only** |
| `OVMF_VARS` | the **NVRAM variable store** (boot entries, settings) | **writable** |

The script locates the code file across macOS/Linux/Windows locations, then copies a **fresh,
writable** vars file to `kaosldr_uefi/ovmf_vars.fd` (the system original stays untouched). A
`CODE→VARS` name derivation keeps the two files matched (same flash size, e.g. both `_4M`).

### Step 4 — Choose the display mode
Selects only *how output is shown* (window vs. serial); irrelevant to booting. See
[`uefi_roadmap.md`](uefi_roadmap.md) and the script comments for `gui` / `serial` / `vnc`.

### Step 5 — Launch QEMU
```bash
qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \  # firmware code as flash
    -drive if=pflash,format=raw,file="$OVMF_VARS" \              # NVRAM (writable)
    -drive format=raw,file=fat:rw:"$ESP_DIR" \                   # esp/ as a FAT "disk"
    "${QEMU_DISPLAY[@]}" -net none -m 256M
```
The three `-drive` lines are the core:

- **`pflash` (CODE + VARS):** maps OVMF as the machine's firmware flash → the VM *is* a UEFI
  machine.
- **`fat:rw:esp` (VVFAT):** QEMU's trick — it presents the host folder `esp/` to the guest as a
  **FAT-formatted disk**, on the fly. No real GPT/ESP image, no `mkfs`, no `dd` needed for
  testing. The firmware sees a FAT disk, finds `/EFI/BOOT/BOOTX64.EFI`, and starts it.

---

## 3. The runtime boot flow

```
┌──────────────────────────────────────────────────────────────┐
│ QEMU starts the virtual machine                               │
└──────────────────────────────────────────────────────────────┘
        │   pflash = OVMF  →  the VM is now a UEFI machine
        ▼
┌──────────────────────────────────────────────────────────────┐
│ OVMF firmware (EDK2) initialises                              │
│   • CPU already in 64-bit long mode                           │
│   • enumerates devices, builds the EFI System Table           │
│   • brings up Boot Services & Runtime Services                │
└──────────────────────────────────────────────────────────────┘
        │   BDS phase: find a bootable medium
        ▼
┌──────────────────────────────────────────────────────────────┐
│ VVFAT drive presented as a FAT volume                         │
│   firmware looks for the fallback path:                       │
│        /EFI/BOOT/BOOTX64.EFI                                  │
└──────────────────────────────────────────────────────────────┘
        │   load the PE32+ image into memory,
        │   apply PE relocations, call the entry point
        ▼
┌──────────────────────────────────────────────────────────────┐
│ efi_main(image_handle, *system_table)   [extern "efiapi"]     │
│   • serial::init()            (COM1, raw port I/O)            │
│   • print(): ConOut->OutputString  AND  COM1                 │
│   • idle loop  (Boot Services still active, no ExitBootSvcs)  │
└──────────────────────────────────────────────────────────────┘
        │
        ▼
   "KAOS UEFI loader: hello from BOOTX64.EFI"     → screen + serial
```

This matches what the serial log shows in practice:
`BdsDxe: starting Boot0001 …` (OVMF loads our application) → `KAOS UEFI loader: hello …`.

### QEMU vs. real hardware

The `fat:rw:` VVFAT drive is a **test-only** convenience. For a USB stick / real hardware
(roadmap step 3), steps 2 and 5 are replaced by a **real GPT image with a FAT ESP**
(`parted` / `mkfs.fat` / `mcopy`), onto which the *same* `BOOTX64.EFI` is copied to the *same*
path. The loader binary is identical — only the "packaging" (VVFAT folder ↔ real disk image)
differs.

---

## 4. Why PE/COFF? UEFI's executable format and its origin

A natural question: UEFI is an open standard, so why is a UEFI binary a **Microsoft PE/COFF**
file? The short answer: this is a **format-level historical legacy, not a software or licensing
dependency** on Microsoft.

### Where it comes from

UEFI did not start out "open". It grew from **Intel's EFI** in the late 1990s, built for
**Itanium (IA-64)** to replace the legacy 16-bit BIOS. Intel needed an executable format for
firmware drivers/applications and chose **PE/COFF**. Only in 2005/2006 did Intel hand EFI to the
vendor-neutral **UEFI Forum** (Intel, AMD, AMI, Apple, Microsoft, …), which produced the open
UEFI standard. The PE/COFF heritage stayed.

### Why PE/COFF rather than ELF

Pragmatic engineering reasons of that era:

- Firmware needs an **easy-to-load, relocatable** format with a clear structure (headers,
  sections, relocations) and a **defined entry point**. PE/COFF had all of this and was very
  well documented and stable.
- PE/COFF has a **"Subsystem" field** in its header. UEFI simply defined new values for it:
  *EFI Application*, *EFI Boot Service Driver*, *EFI Runtime Driver* — a perfect conceptual fit.
- ELF was the alternative but is more tied to Unix/SysV-ABI conventions; PE/COFF was the more
  OS-neutral, simpler-to-load container for firmware.

### It is not really a "Microsoft" lineage

```
COFF  (Unix System V, 1980s — "Common Object File Format")
  └─> PE / "Portable Executable"   (Microsoft's extension of COFF)
        └─> PE32+ subset RE-SPECIFIED by the open UEFI specification
               • Machine    = x86-64
               • Subsystem  = EFI Application / Boot Service Driver / Runtime Driver
               • MS x64 calling convention  ──>  Rust  extern "efiapi"
```

1. **COFF is not Microsoft's** — "Common Object File Format" comes from Unix System V. **PE** is
   merely Microsoft's extension of COFF. The root is Unix.
2. **The UEFI standard re-specifies the exact subset itself.** Which `Machine` types, which
   `Subsystem` values, and which relocation kinds are valid is written in the **public UEFI
   specification** (UEFI Forum) — not in any Microsoft document. You need **nothing** from
   Microsoft: no tools, no library, no license.
3. **Open toolchains emit PE/COFF natively.** Exactly as observed here: Rust + LLVM (`rust-lld`)
   build `bootx64.efi` with no Microsoft component. GNU binutils (`objcopy`) can do it too.

### A second "MS legacy" point: the calling convention

UEFI also adopted the **Microsoft x64 calling convention** (arguments in RCX/RDX/R8/R9, plus
stack "shadow space"), not the System-V convention Linux uses. That is why `efi_main` is declared
`extern "efiapi"`: it tells Rust to use the UEFI/MS ABI for that function. This too is only a
*convention* fixed by the UEFI standard — not Microsoft code.

### Bottom line

The "dependency on Microsoft" is a **historical format envelope** that the standard fully
documents itself and that open tools handle without any Microsoft involvement. Functionally you
are bound to nothing from Microsoft — PE/COFF is just the chosen wrapper for UEFI, the way ELF is
the wrapper for Linux programs. The KAOS loader is 100 % Rust + LLVM and therefore entirely
Microsoft-free despite being a PE/COFF file.

---

## 5. Summary

- `cargo build` emits one artifact: `bootx64.efi`, a PE32+ "EFI Application" with entry
  `efi_main` (`extern "efiapi"`) — not a disk image.
- `build_uefi.sh` then: builds it → stages it at `esp/EFI/BOOT/BOOTX64.EFI` (the UEFI fallback
  path) → obtains OVMF code + a writable vars copy → launches QEMU with OVMF as `pflash` and the
  `esp/` folder as a VVFAT FAT disk.
- At runtime: QEMU+OVMF initialise UEFI → the firmware finds `/EFI/BOOT/BOOTX64.EFI` on the FAT
  volume → loads the PE32+ image and calls `efi_main` → the loader prints to ConOut and COM1.
- The PE/COFF format is a historical legacy from Intel's Itanium-era EFI; the UEFI spec
  re-specifies the subset openly, COFF itself is of Unix origin, and open toolchains produce it
  with zero Microsoft dependency. The same applies to the `efiapi` (MS x64) calling convention.
