# The KAOS UEFI Loader — Build & Boot Pipeline, and the PE/COFF Format

This document explains, in detail, **how the `kaosldr_uefi` loader is built and booted** and
**why a UEFI executable is a Microsoft PE/COFF file** even though UEFI is an open standard.

For the strategic *why/when* of the UEFI migration and the overall roadmap, see
[`uefi_roadmap.md`](uefi_roadmap.md). This document is the concrete, mechanical companion to it.

---

## Table of Contents
- [1. What `cargo build` produces](#1-what-cargo-build-produces)
- [2. The build & image pipeline](#2-the-build--image-pipeline)
- [3. The runtime boot flow — from power-on to a running kernel](#3-the-runtime-boot-flow--from-power-on-to-a-running-kernel)
  - [3.0 The big picture](#30-the-big-picture)
  - [3.1 What `efi_main` does, in order](#31-what-efi_main-does-in-order)
  - [3.2 Loading the kernel at physical `0x100000`](#32-loading-the-kernel-at-physical-0x100000)
  - [3.3 Reserving the PMM-metadata region](#33-reserving-the-pmm-metadata-region)
  - [3.4 Disabling the firmware watchdog](#34-disabling-the-firmware-watchdog)
  - [3.5 `ExitBootServices` and the unified memory map](#35-exitbootservices-and-the-unified-memory-map)
  - [3.6 Creating the higher half: mirror `PML4[0]` → `PML4[256]`](#36-creating-the-higher-half-mirror-pml40--pml4256)
  - [3.7 The hand-off to the kernel](#37-the-hand-off-to-the-kernel)
  - [3.8 What the kernel does — `KernelMain` initialization](#38-what-the-kernel-does--kernelmain-initialization)
  - [3.9 Why `vmm::init` clones the firmware page tables (the hard-won lesson)](#39-why-vmminit-clones-the-firmware-page-tables-the-hard-won-lesson)
  - [Real hardware](#real-hardware)
- [4. Why PE/COFF? UEFI's executable format and its origin](#4-why-pecoff-uefis-executable-format-and-its-origin)
- [5. Summary](#5-summary)

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
It is just the bare executable. Turning it into a bootable medium is the job of `build_uefi_debug.sh`
(§2).

---

## 2. The build & image pipeline

`build_uefi_debug.sh` turns the loader into a **single, real bootable disk image** that is used both
for the QEMU test and for real hardware (no separate "test vs. flash" artifacts):

```
  kaosldr_uefi/src/{main.rs, serial.rs}
            │
            │  cargo build   (target = x86_64-unknown-uefi, linker = rust-lld)
            ▼
  target/x86_64-unknown-uefi/debug/bootx64.efi      ← PE32+ "EFI Application"
            │
            │  build_uefi_debug.sh  (dd + sgdisk + mtools, no root)
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

## 3. The runtime boot flow — from power-on to a running kernel

This section is the heart of the document: it walks, step by step, through everything that
happens from the moment the firmware loads `BOOTX64.EFI` until the Rust kernel is running in its
own address space. It assumes **no prior knowledge** of UEFI, paging, or this codebase.

### 3.0 The big picture

```
┌──────────────────────────────────────────────────────────────┐
│ Firmware (OVMF in QEMU, vendor UEFI on real HW)               │
│   • CPU already in 64-bit long mode, paging ON                │
│   • firmware's own page tables identity-map RAM (phys==virt)  │
│   • Boot Services + Runtime Services available                │
└──────────────────────────────────────────────────────────────┘
        │  finds /EFI/BOOT/BOOTX64.EFI on the FAT32 ESP, loads + calls it
        ▼
┌──────────────────────────────────────────────────────────────┐
│ efi_main()  — the KAOS UEFI loader (kaosldr_uefi)             │
│   1. locate SimpleFileSystem + GOP framebuffer                │
│   2. load KERNEL.BIN to physical 0x100000 (AllocateAddress)   │
│   3. reserve a PMM-metadata region sized to RAM               │
│   4. fill the BootInfo struct (fb, memory map, sizes)         │
│   5. SetWatchdogTimer(0)  — disable the firmware watchdog     │
│   6. ExitBootServices()   — take ownership of the machine     │
│   7. mirror PML4[0] → PML4[256]  (create the higher half)     │
│   8. jump to the kernel:  RSP=0x400000, RDI=&BootInfo,        │
│                           RIP=0xFFFF800000100000              │
└──────────────────────────────────────────────────────────────┘
        │  (firmware page tables still active; kernel runs in the higher half)
        ▼
┌──────────────────────────────────────────────────────────────┐
│ KernelMain(boot_info_ptr)  — the Rust kernel                  │
│   zero BSS → serial → GDT → FPU → PMM → IDT/PIC →             │
│   vmm::init (CLONE firmware PML4 + recursive map, switch CR3) │
│   → heap → PCI → timer → (GOP boot) black/white heartbeat     │
└──────────────────────────────────────────────────────────────┘
```

Two facts about the **environment the loader inherits** are essential for everything below and
are easy to miss:

- **The CPU is already in 64-bit long mode with paging enabled.** UEFI does this before our code
  runs. We never switch the CPU into long mode (unlike the legacy BIOS path, which does it by
  hand in `kaosldr_16/longmode.asm`).
- **The firmware's page tables identity-map physical memory** (virtual address == physical
  address) for low memory / all RAM. So while those tables are active, *a physical address can be
  used directly as a pointer*. The loader and the early kernel both rely on this.

### 3.1 What `efi_main` does, in order

`efi_main(image_handle, system_table)` (in `kaosldr_uefi/src/main.rs`, declared
`extern "efiapi"` so it uses the UEFI/MS-x64 ABI — see §4) runs these steps. Each "protocol" is
just a vtable of function pointers the firmware hands out via `BootServices->HandleProtocol` /
`LocateProtocol`.

1. **Serial** (`serial::init`) — bring up COM1 with raw port I/O so the loader can log even when
   there is no screen output yet.
2. **LoadedImage protocol** on our own `image_handle` → gives us the `device_handle` we were
   loaded from (the USB stick / virtual disk).
3. **SimpleFileSystem protocol** on that device → lets us open files on the FAT32 ESP.
4. **Graphics Output Protocol (GOP)** → the linear framebuffer. We read its **physical base
   address, byte size, width, height, and `pixels_per_scanline`** (the stride, which can be wider
   than `width`) and store them in `BootInfo.fb_info`. This must be done *before*
   `ExitBootServices`, because GOP is a boot service.
5. **Load the kernel** (`KERNEL.BIN`) — see §3.2.
6. **Reserve the PMM-metadata region** — see §3.3.
7. **Disable the watchdog** — see §3.4.
8. **`ExitBootServices`** and build the unified memory map — see §3.5.
9. **Create the higher-half mapping** — see §3.6.
10. **Jump to the kernel** — see §3.7.

### 3.2 Loading the kernel at physical `0x100000`

`KERNEL.BIN` is a **flat binary** (an ELF stripped to raw bytes with `objcopy -O binary`). It is
linked (see `kernel/link.ld`) to run at the **higher-half virtual address
`0xFFFF800000100000`**, but its **physical load address is `0x100000`** (the classic 1 MiB mark).

The loader opens `KERNEL.BIN`, measures its size by seeking to the end, then allocates memory and
reads it in:

```rust
let mut kernel_addr: u64 = 0x100000;
// AllocateType = AllocateAddress (2): allocate at EXACTLY this address.
// MemoryType   = EfiLoaderCode (1).
// pages = 768  →  768 * 4 KiB = 3 MiB, covering 0x100000 .. 0x400000.
allocate_pages(2, 1, 768, &mut kernel_addr);
read(kernel_file, &mut size, 0x100000 as *mut c_void);
```

Two subtleties that previously caused real-hardware-only crashes (now fixed) — keep them in mind:

- **`AllocateType` must be `AllocateAddress` (2), not `AllocateMaxAddress` (1).** With `1`, the
  firmware treats `kernel_addr` as a *ceiling* and may place the kernel anywhere below it (it once
  landed at `0x40000`), so the higher-half mapping then pointed at the wrong physical bytes.
- **768 pages (3 MiB) are reserved, not just the ~90 pages the image occupies.** The kernel later
  places things at fixed low addresses *beyond* its image — the bootstrap **stack** grows down
  from `0x400000`, and on the BIOS path the PMM bitmaps sit just past the BSS. Reserving the whole
  `0x100000..0x400000` block (marked `EfiLoaderCode` in the memory map) keeps all of that inside
  one region the firmware will not reuse.

### 3.3 Reserving the PMM-metadata region

The kernel's Physical Memory Manager (PMM, see [`pmm.md`](pmm.md)) needs a **bitmap** with one bit
per 4 KiB frame of RAM. That bitmap scales with installed memory: ~32 KiB per GiB, so **128 GiB
of RAM needs ~4 MiB of bitmap**. Placing that right after the kernel image would overrun the 3 MiB
low block and scribble over firmware-owned low memory — which triple-faulted on the real 128 GiB
machine (QEMU hid it because OVMF keeps its structures elsewhere).

So the loader **reserves a dedicated metadata region** while boot services are still alive. It
does an initial `GetMemoryMap`, sums the usable frames, sizes the region, and allocates it with
`AllocatePages(AllocateAnyPages, EfiLoaderData, …)` — meaning *the firmware picks any free spot*,
which on a large-RAM box is typically tens of GiB up. The base and size are passed to the kernel
via `BootInfo.pmm_metadata_base` / `pmm_metadata_size`. The PMM then puts its header + region
array + bitmaps there instead of in cramped low memory.

### 3.4 Disabling the firmware watchdog

UEFI arms a **watchdog timer** (~5 minutes) before launching a boot application; if it is left
running it will reset the machine. A proper OS loader disables it:

```rust
set_watchdog_timer(0, 0, 0, core::ptr::null());   // timeout 0 = disabled
```

This must happen *before* `ExitBootServices` (it is a boot service).

### 3.5 `ExitBootServices` and the unified memory map

`ExitBootServices` is the hand-over point: after it returns successfully, **the firmware's Boot
Services are gone** (no more `AllocatePages`, file I/O, GOP calls) and the OS owns the hardware.
The catch: you must pass the *current* `map_key` from a `GetMemoryMap` that nothing has
invalidated since — so it is done in a small retry loop (get map → try exit → on failure re-fetch
and retry).

Just before exiting, the loader walks the UEFI memory map one last time and copies it into a
simple, firmware-independent array (`UnifiedMemoryEntry { start, size, is_usable }`), marking
`EfiConventionalMemory` (type 7) regions as usable. It records this array's address/length, the
kernel size, and the framebuffer info in the shared **`BootInfo`** struct.

#### The `BootInfo` contract

`BootInfo` is the **only channel** between loader and kernel. Its layout is duplicated, field for
field, in three places that must stay in sync: `kaosldr_uefi/src/main.rs`, `kernel/src/boot_info.rs`,
and `kaosldr_64/src/boot_info.rs` (the BIOS loader).

```rust
#[repr(C)]
struct BootInfo {
    magic: u64,                 // 0x4B414F535F424F4F  ("KAOS_BOO") — sanity check
    video_type: VideoModeType,  // 0 = VgaText (BIOS), 1 = GopFramebuffer (UEFI)
    fb_info: FramebufferInfo,   // base_address, size, width, height, pixels_per_scanline
    memory_map_addr: u64,       // pointer to the UnifiedMemoryEntry[] array
    memory_map_len: u32,        // number of entries
    kernel_size: u64,           // bytes of KERNEL.BIN actually read
    pmm_metadata_base: u64,     // reserved PMM-metadata region (0 on BIOS path)
    pmm_metadata_size: u64,
}
```

The kernel verifies `magic` before trusting the pointer (see §3.8), which also lets the same
`KernelMain` work for older loaders / tests that pass a raw integer instead of a pointer.

### 3.6 Creating the higher half: mirror `PML4[0]` → `PML4[256]`

The kernel is *linked* at `0xFFFF800000100000` but *loaded* at physical `0x100000`. For the jump
to that high virtual address to work, the higher half must be mapped **before** we jump — while
the firmware's page tables are still active. The loader does this with a tiny, surgical edit to
the firmware's own top-level table:

```rust
let cr3 = read_cr3();
let pml4 = (cr3 & 0x000F_FFFF_FFFF_F000) as *mut u64;   // phys == virt (identity map)
// temporarily clear CR0.WP so we may write the (write-protected) page table
*pml4.add(256) = *pml4.add(0);   // copy entry 0 to entry 256
// restore CR0.WP, then reload CR3 to flush the TLB
```

Why entry **256**? A virtual address is split into 9-bit indices; bits 47..39 select the PML4
slot. `0xFFFF800000000000 >> 39 & 0x1FF = 256`. Entry 0 maps virtual `0x0…` (the firmware's
identity map of low RAM); copying it to entry 256 makes virtual `0xFFFF800000000000…` resolve to
the **same** physical pages. So `0xFFFF800000100000` now points at physical `0x100000` — exactly
where the kernel image sits. (This mirror is later inherited by the kernel; see §3.9.)

### 3.7 The hand-off to the kernel

With everything prepared, the loader jumps. It does **not** `call` — it sets up the exact CPU
state the kernel's `extern "C"` entry point expects and `jmp`s:

```rust
asm!(
    "mov rsp, 0x400000",   // bootstrap stack top (grows down, inside the 3 MiB low block)
    "xor rbp, rbp",        // clear frame pointer
    "jmp {entry}",         // entry = 0xFFFF800000100000  (higher-half KernelMain)
    in("rdi") &BOOT_INFO,  // SysV ABI: first argument in RDI = pointer to BootInfo
);
```

So at the instant the kernel starts: **RIP** is in the higher half, **RSP** is the low identity
address `0x400000`, **RDI** holds the `BootInfo` pointer, and the **firmware's page tables are
still active** (with our higher-half mirror added).

### 3.8 What the kernel does — `KernelMain` initialization

`KernelMain(boot_info_raw)` (`kernel/src/main.rs`, placed first in `.text.boot` by the linker so
it is the entry) runs this sequence:

1. **`zero_bss()`** — physical RAM is not guaranteed zeroed on real hardware (QEMU happens to zero
   it), so every zero-initialized static would otherwise hold garbage. This must run first.
2. **`serial::init`**, then check the `BootInfo` **magic** at `boot_info_raw`; if valid, publish
   the pointer in a global (`BOOT_INFO_PTR`) and, on a GOP boot, paint a color gradient so you can
   see the kernel is alive.
3. **`gdt::init`** — Global Descriptor Table + TSS. Also reloads `CS` via a far return, because
   UEFI hands off `CS=0x38`, which has no descriptor in our GDT (the BIOS path happened to hand
   off `CS=0x08`). Skipping this faults on the first `iretq`.
4. **`fpu::init`** — enable SSE/FPU and capture the default FPU state.
5. **`pmm::init`** — build the Physical Memory Manager from `BootInfo`'s memory map, placing its
   metadata in the reserved region (§3.3). See [`pmm.md`](pmm.md).
6. **`interrupts::init`** — Interrupt Descriptor Table + 8259 PIC remap (handlers installed,
   interrupts still *disabled*).
7. **`vmm::init`** — **the critical step.** It builds the kernel's own top-level page table as a
   **superset of the firmware's** (it *clones* the firmware `PML4` and adds a recursive self-map),
   then switches `CR3` to it. A naïve "build a minimal map from scratch" approach reset real AMD
   hardware instantly here; see §3.9 and [`vmm.md`](vmm.md).
8. **`heap::init`**, **`drivers::pci::init`**, **`drivers::time::init`** — kernel heap, PCI bus
   scan, timer.
9. **End state:** on a **GOP/UEFI** boot the kernel currently stops here in a steady
   **black ↔ white framebuffer heartbeat** (a visible "I booted and I'm alive" signal). The
   disk-dependent path below it — ATA PIO, the FAT12 file system, loading the user-space shell,
   the scheduler — is **skipped on UEFI**, because a USB/UEFI boot has no legacy ATA disk yet.
   The **legacy BIOS/VGA** boot (`video_type == VgaText`) instead continues into that disk +
   scheduler path unchanged.

### 3.9 Why `vmm::init` clones the firmware page tables (the hard-won lesson)

The intuitive design is: have the kernel build its *own* minimal page tables (identity-map the
low few MiB, map the higher half, add a recursive entry) and switch to them. That worked in QEMU
but **instantly reset the real AMD machine the moment `CR3` was loaded** — with no CPU exception
of any kind, so nothing could be caught or printed.

The reason (best-supported explanation): real firmware leaves **System Management Mode (SMM)**
active and takes **asynchronous SMIs** (for power/thermal/USB-legacy emulation). The platform's
SMM/firmware path depends on the memory mappings the firmware set up. A minimal kernel map
*discards* those mappings, so the next SMI faults inside SMM and the platform hard-resets. QEMU
has no such SMM activity, which is exactly why it tolerated the minimal map.

The fix is to **never discard the firmware's mappings**. Because the firmware tables are still
active when `vmm::init` runs, the kernel reads `CR3`, copies all 512 entries of the firmware's
top-level `PML4` into a fresh frame, and only then overwrites slot 511 with its own recursive
self-map. The result keeps the firmware's full identity map, the higher-half mirror from §3.6, and
all SMM/ACPI/MMIO/runtime regions — and adds the recursive window the VMM needs. The full
mechanics, the bisection that proved it, and the remaining caveats are documented in
[`vmm.md`](vmm.md).

#### Background: what are SMM and SMIs?

If "SMM/SMI" means nothing to you, here is the minimum needed to understand the reset above.

**SMM — System Management Mode** is a *separate, highly privileged CPU mode*, alongside real
mode, protected mode, and long mode. It is *more* privileged than the kernel — it is often called
**"ring −2"**:

- Its code runs from a protected memory region called **SMRAM** (on modern systems "TSEG",
  typically just below the top of RAM) that is locked away from normal accesses — even the kernel
  cannot read or write it.
- The code inside (the **SMI handler**) belongs to the **firmware** and is installed once at boot.
- SMM has its **own environment** (its own setup and entry state), independent of the operating
  system's page tables and IDT.

**SMI — System Management Interrupt** is the *only* way to enter SMM. It is a hardware interrupt
that:

- is **non-maskable** — even stronger than an NMI; `cli` cannot block it, and
- does **not** go through the IDT. On an SMI the CPU instead (1) saves the *entire* CPU state
  (registers, CR3, …) into SMRAM, (2) switches into SMM and runs the firmware's SMI handler, then
  (3) executes `RSM` (resume) to restore the saved state and return — transparently, as if nothing
  happened.

**"Asynchronous"** means the SMI fires by itself at unpredictable times, triggered by
hardware/chipset — not requested by the OS and invisible to it. Typical triggers on real hardware:
power/thermal management (clock, fan, temperature, battery), **USB-legacy emulation** (the firmware
fakes a PS/2 keyboard/mouse for an attached USB device, so a keypress can raise an SMI), ACPI, TPM,
and error handling. These SMIs occur **periodically in the background on real hardware** — but
**essentially never in QEMU** (without special configuration). That alone explains why the bug was
real-hardware-only.

This is why the symptoms fit SMM exactly: a reset with **no visible CPU exception** (an SMI does
not use the IDT, so our catch-all on vectors 0–31 saw nothing), **not a `#MC`**, and **only on real
hardware**. Privilege ladder for orientation:

| Level         | Who                | Via the IDT?            | Masked by `cli`? |
|---------------|--------------------|-------------------------|------------------|
| Ring 3        | user programs      | –                       | –                |
| Ring 0        | the kernel         | –                       | –                |
| NMI           | critical HW errors | yes (vector 2)          | no               |
| **SMI → SMM** | **the firmware**   | **no** (own path/SMRAM) | **no**           |

> *Honest caveat:* that SMM is the precise trigger is the **best-supported hypothesis** — it
> explains every symptom, but the exact micro-trigger *inside* SMM could not be observed directly
> (SMRAM is not externally inspectable). What is *proven* is: minimal map → reset, firmware clone
> → stable.

### Real hardware

Because `build_uefi_debug.sh` already produces a real GPT/ESP image, real hardware needs **no
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

### Real-hardware smoke-test checklist

The most dangerous UEFI failure mode — the firmware's asynchronous SMM/SMI path faulting the
instant `vmm::init` loads CR3 (§3.9) — is **not reproducible in QEMU or any unit test**. It only
appears on real hardware. So real-HW validation is a manual, but **repeatable and recorded**,
step. Run this checklist after any change to the page-table / PMM / CR3-switch code:

1. **Build the image and QEMU pre-flight in one step** (`build_uefi_debug.sh` produces the GPT/ESP
   image *and* boots it in QEMU under OVMF — the same artifact you flash, see §2):
   ```bash
   ./build_uefi_debug.sh          # builds kaos64-uefi.img and launches QEMU+OVMF
   ```
   Expect the markers in step 4 below. A QEMU pass does **not** prove the SMM path is safe —
   only real hardware does — but a QEMU failure means don't bother flashing yet.
2. **Flash to USB** (DESTRUCTIVE — confirm the device node first with `lsblk`):
   ```bash
   sudo dd if=kaos64-uefi.img of=/dev/<your-usb> bs=4M conv=fsync
   ```
3. **Boot the UEFI target**: Secure Boot **off**, CSM/legacy **off**, boot from the stick.
   (Confirm the machine is UEFI-capable — see the note above.) Attach a serial adapter if the
   board has one; the per-phase `debugln!` markers come out there.
4. **Expected, in order** (this is the pass criterion):
   - serial: `Unified BootInfo structure detected!`, then `Kernel size: …`, `BootInfo memory map len: …`;
   - screen: a **full-screen color gradient** (red left→right, green top→bottom) — "the kernel is
     executing from the loaded image";
   - serial: the init markers `GDT/TSS initialized` → `FPU/SSE subsystem initialized` →
     `Physical Memory Manager initialized` → `Firmware page-table frames reserved` →
     `Interrupt subsystem initialized` → **`Virtual Memory Manager initialized`** (this line prints
     *after* the CR3 switch survived — the critical SMM checkpoint) → `Heap Manager initialized` →
     `PCI subsystem initialized` → `Time driver initialized`;
   - screen: a steady **black ↔ white framebuffer heartbeat** — "booted, survived the CR3 switch,
     and the timer/IRQs are alive". (The disk/scheduler path is UEFI-skipped for now — §3.8.)
5. **Interpreting a failure:**
   - **Screen freezes on the gradient and the machine resets/reboots right around the CR3 switch**,
     and serial never reaches `Virtual Memory Manager initialized` → the SMM/SMI regression is back
     (§3.9). The kernel PML4 is no longer a faithful superset of the firmware tables. Re-check
     `build_kernel_pml4_from_firmware` and the firmware-frame reservation.
   - **Gradient never appears** → the loader/hand-off or higher-half mirror is broken (§3.6–3.7),
     not the CR3 switch.
   - **Reaches the markers but no heartbeat** → init completed but the timer/IRQ path stalled;
     unrelated to the SMM lesson.

Record the date, the commit hash, the exact board (UEFI vendor/version), and which step was
reached, so each HW run is an auditable data point.

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
- `build_uefi_debug.sh` then: builds it → assembles a real GPT disk image `kaos64-uefi.img` with a
  FAT32 EFI System Partition holding `/EFI/BOOT/BOOTX64.EFI` (`dd` + `sgdisk` + `mtools`, no
  root) → obtains OVMF code + a writable vars copy → boots that image in QEMU with OVMF as
  `pflash`.
- The same `kaos64-uefi.img` is what you `dd` to a USB stick for real (UEFI-capable) hardware —
  QEMU test and hardware run identical bytes. Legacy-BIOS-only machines cannot boot it.
- At runtime (see §3 for the step-by-step): firmware finds `/EFI/BOOT/BOOTX64.EFI` on the FAT ESP
  and calls `efi_main` → the loader locates GOP, loads `KERNEL.BIN` to physical `0x100000`,
  reserves a PMM-metadata region, fills `BootInfo`, disables the watchdog, calls
  `ExitBootServices`, mirrors `PML4[0]→PML4[256]` to create the higher half, and jumps to the
  kernel at `0xFFFF800000100000` (RSP=`0x400000`, RDI=`&BootInfo`). The kernel then inits
  GDT/FPU/PMM/IDT, and in `vmm::init` **clones the firmware PML4** (+ a recursive self-map) before
  switching CR3 — a minimal hand-built map reset real AMD hardware. On a GOP boot it ends in a
  black↔white framebuffer heartbeat (the disk/scheduler path is UEFI-skipped for now).
- The PE/COFF format is a historical legacy from Intel's Itanium-era EFI; the UEFI spec
  re-specifies the subset openly, COFF itself is of Unix origin, and open toolchains produce it
  with zero Microsoft dependency. The same applies to the `efiapi` (MS x64) calling convention.
