# KAOS — A 64-bit Operating System Written in Rust

KAOS is an experimental, educational x86_64 operating system written from scratch in
**Rust** (`#![no_std]`). It boots on both legacy **BIOS** and modern **UEFI** firmware,
runs the kernel in the higher half of the address space, schedules preemptive
multitasking, and launches unprivileged **Ring-3** user programs — including an
interactive shell — from a FAT file system.

The project is built with **zero external crate dependencies** (only `core` and `alloc`):
every subsystem — the physical/virtual memory managers, the heap allocator, the
scheduler, the synchronization primitives, the storage stack and filesystems, the PCI
and AHCI drivers, and the console — is implemented in-tree. It runs in QEMU and on real
hardware.

> **Background & write-ups:** <https://www.SQLpassion.at/archive/category/osdev/>

---

## Highlights

- **Pure Rust kernel** in Ring 0, `#![no_std]`, no third-party crates.
- **Dual boot path:** a three-stage BIOS boot chain *and* a UEFI (PE/COFF) loader.
- **Higher-half kernel** mapped at `0xFFFF_8000_0000_0000` on x86_64 long mode.
- **Physical Memory Manager** — bitmap-based 4 KiB page-frame allocator.
- **Virtual Memory Manager** — 4-level paging, per-process address spaces, page-fault handling.
- **Heap allocator** — segregated free-list with 32 bins and an O(1) bitmap lookup.
- **Preemptive round-robin scheduler** — PIT-driven context switching, lazy FPU/SSE state switching.
- **System calls** via `int 0x80`, with safe Ring-3 user-space wrappers.
- **Storage stack** — ATA PIO + AHCI/SATA block devices, GPT partitions, FAT12 & FAT32, and a single-mount VFS.
- **Console subsystem** — runtime-polymorphic VGA text-mode *and* linear graphics framebuffer backends.
- **PCI bus driver** with BAR sizing and device enumeration.
- **Text User Interface (TUI)** framework with multi-tab windowing.
- **Custom `no_std` test framework** that boots each integration test as its own kernel in QEMU.

---

## Boot Flow at a Glance

```
BIOS path                                   UEFI path
─────────                                   ─────────
BIOS POST                                   UEFI firmware
   │                                           │
   ▼                                           ▼
Stage 1: bootsector.asm  (16-bit real mode)   kaosldr_uefi  (BOOTX64.EFI, PE/COFF)
   │  loads KLDR16.BIN                         │  loads kernel, builds memory map,
   ▼                                           │  ExitBootServices, sets up paging
Stage 2: KLDR16.BIN      (real → protected)    │
   │  loads KLDR64.BIN                         │
   ▼                                           │
Stage 3: KLDR64.BIN      (long mode, paging)   │
   │                                           │
   └───────────────────►  kernel.bin  ◄────────┘
                        (higher-half Rust kernel)
                              │
                              ▼
                   Ring-3 shell + user programs
```

See [`boot_bios.md`](main64/docs/boot_bios.md) and [`boot_uefi.md`](main64/docs/boot_uefi.md)
for the full mechanics.

---

## Repository Layout

```
osdev/
├── main64/                  # The operating system (Cargo workspace)
│   ├── kernel/              # The Rust kernel (crate: kernel)
│   │   └── src/
│   │       ├── arch/        # x86_64: GDT/TSS, IDT, interrupts, FPU, MSR, ports
│   │       ├── memory/      # PMM, VMM, heap allocator
│   │       ├── scheduler/   # Round-robin preemptive scheduler
│   │       ├── sync/        # SpinLock & scheduler-aware wait queues
│   │       ├── syscall/     # int 0x80 dispatch, ABI, user-pointer checks
│   │       ├── drivers/     # PCI, AHCI, ATA, keyboard, serial, screen, timer
│   │       ├── io/          # GPT, VFS, FAT12, FAT32
│   │       ├── console/     # VGA text-mode + framebuffer backends
│   │       └── process/     # Process types and the user-program loader
│   ├── kaosldr_16/          # Stage 1+2 BIOS loaders (assembly)
│   ├── kaosldr_64/          # Stage 3 long-mode loader (Rust)
│   ├── kaosldr_uefi/        # UEFI loader → BOOTX64.EFI (Rust)
│   ├── lib_kaos/            # Ring-3 user-space runtime / syscall library
│   ├── lib_tui/             # User-space TUI library
│   ├── user_programs/       # Ring-3 programs: shell, hello, readline, kbasic, filedemo, tui_app
│   ├── docs/                # Subsystem documentation (see below)
│   └── build*.sh            # Build & image-generation scripts
├── docker-buildenv/         # Dev-container build environment
├── LICENSE                  # MIT
└── README.md
```

---

## Building & Running

The OS is a Cargo workspace under `main64/` and requires the **Rust nightly** toolchain
(the kernel relies on nightly features and `#![no_std]`).

> All commands below are run from the `main64/` directory.

### BIOS image (QEMU)

```bash
# macOS (host)
./build_kaos_debug.sh   && qemu-system-x86_64 -drive format=raw,file=../kaos64.img --serial stdio
./build_kaos_release.sh && qemu-system-x86_64 -drive format=raw,file=../kaos64.img --serial stdio

# Inside the dev container
./build.sh && qemu-system-x86_64 -drive format=raw,file=kaos64.img -display curses
```

### UEFI image (QEMU + OVMF)

```bash
./build_uefi.sh
```

This produces a GPT disk image (`kaos64-uefi.img`) with a FAT32 EFI System Partition
containing `/EFI/BOOT/BOOTX64.EFI`. The same image can be written 1:1 to a USB stick and
booted on real UEFI hardware. Requires QEMU + OVMF, `gptfdisk` (`sgdisk`) and `mtools`
(`brew install qemu gptfdisk mtools` on macOS; preinstalled in the dev container).

### Running the test suite

```bash
cargo test -p kernel    # from main64/
```

Each integration test is compiled into a standalone kernel binary that boots in QEMU and
signals pass/fail via the `isa-debug-exit` device. See [`testing.md`](main64/docs/testing.md).

### Running on real hardware

Write the final FAT12 image to a physical disk, e.g. on macOS:

```bash
sudo dd if=kaos64.img of=/dev/diskN   # where diskN is the target disk
```

---

## Documentation

Detailed, implementation-level documentation for each subsystem lives in
[`main64/docs/`](main64/docs/).

### Boot & loaders
| Document | Topic |
|---|---|
| [boot_bios.md](main64/docs/boot_bios.md) | Three-stage BIOS boot process (real → protected → long mode) |
| [boot_uefi.md](main64/docs/boot_uefi.md) | UEFI loader, the PE/COFF format, and the boot pipeline |
| [loader.md](main64/docs/loader.md) | Loading a Ring-3 user program from FAT12 into its own address space |
| [gdt.md](main64/docs/gdt.md) | GDT/TSS, privilege transitions, and why they matter in long mode |

### Memory
| Document | Topic |
|---|---|
| [pmm.md](main64/docs/pmm.md) | Physical Memory Manager (bitmap page-frame allocator) |
| [vmm.md](main64/docs/vmm.md) | Virtual Memory Manager (4-level paging, page faults) |
| [heap.md](main64/docs/heap.md) | Segregated free-list heap allocator |

### Execution & concurrency
| Document | Topic |
|---|---|
| [scheduling.md](main64/docs/scheduling.md) | Preemptive multitasking via timer-driven context switching |
| [sync.md](main64/docs/sync.md) | Synchronization primitives & scheduler-aware wait queues |
| [timer.md](main64/docs/timer.md) | High-precision timekeeping (PIT + TSC) and the timer driver |
| [syscall.md](main64/docs/syscall.md) | The `int 0x80` system-call path, end to end |

### Devices & storage
| Document | Topic |
|---|---|
| [pci.md](main64/docs/pci.md) | PCI bus driver, configuration space, BAR sizing |
| [storage.md](main64/docs/storage.md) | Storage stack: ATA/AHCI → block device → GPT → FAT → VFS |
| [fat12.md](main64/docs/fat12.md) | FAT12 file system design & usage |
| [console.md](main64/docs/console.md) | Console subsystem: VGA text-mode and framebuffer backends |
| [tui.md](main64/docs/tui.md) | Text User Interface framework architecture |

### Testing
| Document | Topic |
|---|---|
| [testing.md](main64/docs/testing.md) | The custom `no_std` kernel test framework |

### Design notes & roadmap
| Document | Topic |
|---|---|
| [storage_abstraction.md](main64/docs/storage_abstraction.md) | Design plan that motivated the unified storage/VFS architecture |
| [drivers.md](main64/docs/drivers.md) | *(Planned)* dynamic, loadable Ring-3 user-space driver infrastructure |
| [todo_uefi_kernel_pagetables.md](main64/docs/todo_uefi_kernel_pagetables.md) | Plan for kernel-owned page tables on the UEFI path |
| [todo_elf.md](main64/docs/todo_elf.md) | Plan for ELF program loading |

---

## License

Released under the [MIT License](LICENSE). Copyright © 2022–2026 Klaus Aschenbrenner,
[www.SQLpassion.at](https://www.SQLpassion.at).
