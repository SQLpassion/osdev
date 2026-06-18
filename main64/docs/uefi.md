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
   expects (see §4).

Build details specific to this project:

- There is **no prebuilt `core` guaranteed** for `x86_64-unknown-uefi` in every environment, so
  the toolchain provides `core`/`compiler_builtins` for the target (installed in the dev
  container image, see `.devcontainer/Dockerfile`). The `rust-src` component backs this.
- **No external crates.** The minimal UEFI structures (System Table, Simple Text Output
  Protocol) and the COM1 serial driver are hand-declared in `src/main.rs` and `src/serial.rs`.

**What `bootx64.efi` is *not*:** it is not a disk image, not a partition, and not a boot sector.
It is just the bare executable. Turning it into a bootable medium is the job of `build_uefi.sh`
(§2).

---

## 2. The build & image pipeline

`build_uefi.sh` turns the loader into a **single, real bootable disk image** that is used both
for the QEMU test and for real hardware (no separate "test vs. flash" artifacts):

```
  kaosldr_uefi/src/{main.rs, serial.rs}
            │
            │  cargo build   (target = x86_64-unknown-uefi, linker = rust-lld)
            ▼
  target/x86_64-unknown-uefi/debug/bootx64.efi      ← PE32+ "EFI Application"
            │
            │  build_uefi.sh  (dd + sgdisk + mtools, no root)
            ▼
  kaos64-uefi.img                                   ← GPT disk image
   ├─ protective MBR + GPT header
   └─ partition 1: EFI System Partition, FAT32 (type ef00)
         └─ /EFI/BOOT/BOOTX64.EFI                   ← UEFI fallback boot path
            │
            │  + OVMF_CODE.fd (firmware, ro)  + ovmf_vars.fd (NVRAM, writable copy)
            ▼
  qemu-system-x86_64  -drive pflash(CODE) -drive pflash(VARS) -drive raw(kaos64-uefi.img) ...
            │
            └───────────────►  same image:  dd → USB stick → real UEFI hardware
```

### Step 1 — Build
`cargo build` in the crate directory → `bootx64.efi` (§1).

### Step 2 — Build the bootable GPT/ESP image (`kaos64-uefi.img`)
A real disk image is assembled, **without root**, using `dd` + `gptfdisk` (`sgdisk`) + `mtools`:

```bash
dd if=/dev/zero of=kaos64-uefi.img bs=1048576 count=128          # 128 MiB backing file
sgdisk --clear \
       --new=1:2048:0 --typecode=1:ef00 --change-name=1:"EFI System Partition" \
       kaos64-uefi.img                                           # GPT + one ESP (type ef00)
mformat -i kaos64-uefi.img@@1M -F ::                             # FAT32 in the partition (@1 MiB)
mmd     -i kaos64-uefi.img@@1M ::/EFI ::/EFI/BOOT                # create /EFI/BOOT
mcopy   -i kaos64-uefi.img@@1M bootx64.efi ::/EFI/BOOT/BOOTX64.EFI
```

This encodes the **UEFI boot convention**: with no extra configuration, a UEFI firmware searches
a FAT-formatted EFI System Partition for the fixed *fallback / removable-media* path
**`/EFI/BOOT/BOOTX64.EFI`** (the x86-64 default). `mtools`' `image@@1M` syntax operates directly
on the partition at byte offset 1 MiB (sector 2048), so no loop device / mount / root is needed.

**Required host tools:** `gptfdisk` (`sgdisk`) and `mtools`. They are preinstalled in the dev
container (`.devcontainer/Dockerfile`); on macOS install them with
`brew install gptfdisk mtools`. The image-file creation uses `dd` (not GNU `truncate`) so it is
portable across Linux and macOS.

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
    -drive format=raw,file="$IMG" \                              # the real GPT/ESP disk image
    "${QEMU_DISPLAY[@]}" -net none -m 256M
```
The three `-drive` lines are the core:

- **`pflash` (CODE + VARS):** maps OVMF as the machine's firmware flash → the VM *is* a UEFI
  machine.
- **`-drive format=raw,file=kaos64-uefi.img`:** attaches the real disk image as a virtual disk.
  QEMU+OVMF therefore boot **exactly the artifact you flash to USB** — the QEMU test and real
  hardware run the same bytes.

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
│ GPT disk (kaos64-uefi.img) → EFI System Partition (FAT32)     │
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

### Real hardware

Because `build_uefi.sh` already produces a real GPT/ESP image, real hardware needs **no
different artifact** — write the very same `kaos64-uefi.img` 1:1 to a USB stick (or SSD):

```bash
sudo dd if=kaos64-uefi.img of=/dev/<your-usb> bs=4M conv=fsync   # DESTRUCTIVE — pick the right device!
```

Then on the target machine: **disable Secure Boot** (the binary is unsigned), **disable
CSM/legacy** (force UEFI), and boot from the stick. On real hardware the firmware renders the
`ConOut` output directly on the monitor, so the "hello" appears on screen (serial output comes in
addition if a serial port/adapter is present).

> **The target must actually support UEFI OS boot.** Pure legacy-BIOS machines (e.g. the
> ThinkPad W510) cannot boot this image: a legacy BIOS runs the GPT *protective MBR*, which holds
> no boot code, so you only get a blinking text-mode caret. Note that enabling AHCI is unrelated
> to UEFI. Check Windows `msinfo32` → "BIOS Mode": `UEFI` is required; `Legacy` means the machine
> cannot UEFI-boot. Use UEFI-capable hardware (~2012+) or QEMU+OVMF for UEFI testing.

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
- `build_uefi.sh` then: builds it → assembles a real GPT disk image `kaos64-uefi.img` with a
  FAT32 EFI System Partition holding `/EFI/BOOT/BOOTX64.EFI` (`dd` + `sgdisk` + `mtools`, no
  root) → obtains OVMF code + a writable vars copy → boots that image in QEMU with OVMF as
  `pflash`.
- The same `kaos64-uefi.img` is what you `dd` to a USB stick for real (UEFI-capable) hardware —
  QEMU test and hardware run identical bytes. Legacy-BIOS-only machines cannot boot it.
- At runtime: QEMU+OVMF initialise UEFI → the firmware finds `/EFI/BOOT/BOOTX64.EFI` on the FAT
  ESP → loads the PE32+ image and calls `efi_main` → the loader prints to ConOut and COM1.
- The PE/COFF format is a historical legacy from Intel's Itanium-era EFI; the UEFI spec
  re-specifies the subset openly, COFF itself is of Unix origin, and open toolchains produce it
  with zero Microsoft dependency. The same applies to the `efiapi` (MS x64) calling convention.
