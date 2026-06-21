# Physical Memory Manager (PMM)

The Physical Memory Manager (PMM) is a core kernel subsystem responsible for tracking, allocating, and freeing physical memory at a granularity of **4 KiB page frames**. 

## Table of Contents
- [1. Design & Core Principles](#1-design--core-principles)
- [2. Physical Memory Layout](#2-physical-memory-layout)
- [3. Data Structures](#3-data-structures)
- [4. Initialization Workflow](#4-initialization-workflow)
- [5. Allocation & Release Algorithms](#5-allocation--release-algorithms)
- [6. Bitwise Operations](#6-bitwise-operations)

---

## 1. Design & Core Principles

- **Granularity**: Operates on 4 KiB pages (matching the page size of x86-64 long mode).
- **Backing Allocator**: Uses a bitmap-based allocation scheme where **1 bit** represents **1 page frame** (0 = Free, 1 = Allocated/Reserved).
- **Placement**: Depends on the boot path (see §4.0 and §4 step 2):
  - **UEFI path**: in a *dedicated region the loader reserved* and sized to installed RAM, passed
    via `BootInfo.pmm_metadata_base` / `pmm_metadata_size`. On large-RAM machines the bitmaps are
    several MiB, far too big to sit in low memory — so the loader allocates this region (typically
    tens of GiB up) **before** `ExitBootServices`.
  - **BIOS path / tests (fallback)**: immediately after the kernel BSS section (`__bss_end`) in
    physical memory, aligned to the next 4 KiB page boundary.
- **Thread Safety**: Access to the allocator is synchronized via a global spinlock wrapper (`GlobalPmm`), which disables interrupts during critical sections to prevent preemption and deadlocks.
- **Zero Dependencies**: Implemented using only core Rust primitives (`core`), in alignment with the repository's dependency policy.

---

## 2. Physical Memory Layout

Below is the layout of the physical memory space showing how the kernel, loader data, PMM metadata structures, and bitmaps are arranged:

```text
                    PHYSICAL MEMORY LAYOUT
    ═══════════════════════════════════════════════════════════════════

    0x00000000 ┌─────────────────────────────────────────────────────┐
               │                                                     │
               │              Real Mode Memory                       │
               │         (IVT, BDA, BIOS Data, etc.)                 │
               │                                                     │
    0x00001000 ├─────────────────────────────────────────────────────┤
               │  BiosInformationBlock (BIB_OFFSET)                  │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ year: i32                                   │    │
               │  │ month: i16                                  │    │
               │  │ day: i16                                    │    │
               │  │ hour: i16                                   │    │
               │  │ minute: i16                                 │    │
               │  │ second: i16                                 │    │
               │  │ memory_map_entries: i16                     │    │
               │  │ max_memory: i64                             │    │
               │  │ available_page_frames: i64                  │    │
               │  │ physical_memory_layout: *mut ──────────────────────┐
               │  └─────────────────────────────────────────────┘    │ │
    0x00001200 ├─────────────────────────────────────────────────────┤ │
               │  BIOS Memory Map (MEMORYMAP_OFFSET)                 │ │
               │  ┌─────────────────────────────────────────────┐    │ │
               │  │ BiosMemoryRegion[0]                         │    │ │
               │  │   start: u64                                │    │ │
               │  │   size: u64                                 │    │ │
               │  │   region_type: u32                          │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ BiosMemoryRegion[1]                         │    │ │
               │  │   ...                                       │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ BiosMemoryRegion[N]                         │    │ │
               │  │   ...                                       │    │ │
               │  └─────────────────────────────────────────────┘    │ │
               │                                                     │ │
               │         ... (reserved/other BIOS areas) ...         │ │
               │                                                     │ │
    0x00100000 ├═════════════════════════════════════════════════════┤ │
  (1 MB)       │                                                     │ │
 KERNEL_OFFSET │                    KERNEL CODE                      │ │
               │                                                     │ │
               │              (.text, .rodata, .data, .bss)          │ │
               │                                                     │ │
               │                   kernel_size bytes                 │ │
               │                                                     │ │
    0x00100000 ├─────────────────────────────────────────────────────┤ │
        +      │                                                     │ │
  kernel_size  │            (padding to 4KB alignment)               │ │
   (aligned)   │                                                     │ │
               ├═════════════════════════════════════════════════════┤◄┘
               │                                                     │
               │          PmmLayoutHeader + PmmRegion[]              │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ region_count: u32 (4 bytes)                 │    │
               │  │ padding: u32 (4 bytes)                      │    │
               │  ├─────────────────────────────────────────────┤    │
               │  │ regions[0]: PmmRegion                       │    │
               │  │   ┌───────────────────────────────────────┐ │    │
               │  │   │ start: u64                            │ │    │
               │  │   │ frames_total: u64                     │ │    │
               │  │   │ frames_free: u64                      │ │    │
               │  │   │ bitmap_start: u64 ─────────────────────────────┐
               │  │   │ bitmap_bytes: u64                     │ │      │
               │  │   └───────────────────────────────────────┘ │    │ │
               │  ├─────────────────────────────────────────────┤    │ │
               │  │ regions[1]: PmmRegion                       │    │ │
               │  │   (same fields as above) ─────────────────────────────┐
               │  ├─────────────────────────────────────────────┤    │ │  │
               │  │                ...                          │    │ │  │
               │  └─────────────────────────────────────────────┘    │ │  │
               ├═══════════════════════════════════════════════════┤◄──┘  │
               │                                                     │    │
               │         BITMAP FOR REGION 0                         │    │
               │  ┌─────────────────────────────────────────────┐    │    │
               │  │ Each bit = 1 page frame (4KB)               │    │    │
               │  │                                             │    │    │
               │  │ Word 0: [bits 0-63]                         │    │    │
               │  │   bit 0: 1=used, 0=free (PFN 0 of region)   │    │    │
               │  │   bit 1: 1=used, 0=free (PFN 1 of region)   │    │    │
               │  │   ...                                       │    │    │
               │  │ Word 1: [bits 64-127]                       │    │    │
               │  │   ...                                       │    │    │
               │  │ Word N: [bits N*64 - (N+1)*64-1]            │    │    │
               │  │                                             │    │    │
               │  │ Size = bitmap_bytes                         │    │    │
               │  └─────────────────────────────────────────────┘    │    │
               ├─────────────────────────────────────────────────────┤◄───┘
               │                                                     │
               │         BITMAP FOR REGION 1                         │
               │  ┌─────────────────────────────────────────────┐    │
               │  │ (same structure as above)                   │    │
               │  │ Size = region[1].bitmap_bytes               │    │
               │  └─────────────────────────────────────────────┘    │
               ├─────────────────────────────────────────────────────┤
               │                                                     │
               │         BITMAP FOR REGION N...                      │
               │                                                     │
               ├═════════════════════════════════════════════════════┤
               │                                                     │
               │                                                     │
               │            FREE PHYSICAL MEMORY                     │
               │                                                     │
               │      (allocatable via alloc_frame())                │
               │                                                     │
               │                                                     │
               └─────────────────────────────────────────────────────┘


    ═══════════════════════════════════════════════════════════════════
                   PageFrame HANDLE (returned by alloc_frame)
    ═══════════════════════════════════════════════════════════════════

         ┌────────────────────────────────────────────────────────┐
         │  PageFrame (12 bytes logical, may be padded)           │
         │  ┌──────────────────────────────────────────────────┐  │
         │  │ pfn: u64 (8 bytes)                               │  │
         │  │   Page Frame Number = phys_addr / 4096           │  │
         │  ├──────────────────────────────────────────────────┤  │
         │  │ region_index: u32 (4 bytes, private)             │  │
         │  │   Index into memory_regions[] array              │  │
         │  └──────────────────────────────────────────────────┘  │
         │                                                        │
         │  Methods:                                              │
         │    physical_address() -> pfn * 4096                    │
         └────────────────────────────────────────────────────────┘


    ═══════════════════════════════════════════════════════════════════
                              BITMAP ENCODING
    ═══════════════════════════════════════════════════════════════════

    Each u64 word in bitmap:
    ┌────┬────┬────┬────┬────┬────┬────┬────┬─────────┬────┬────┬────┐
    │ 63 │ 62 │ 61 │ 60 │ 59 │ 58 │ 57 │ 56 │   ...   │  2 │  1 │  0 │
    └────┴────┴────┴────┴────┴────┴────┴────┴─────────┴────┴────┴────┘
      │                                                            │
      │    1 = Page frame ALLOCATED (in use)                       │
      │    0 = Page frame FREE (available)                         │
      │                                                            │
      └── bit 63 = PFN (word_index * 64 + 63)                      │
                                           bit 0 = PFN (word_index * 64 + 0)

    Example: For region starting at 0x100000 (1MB):
      Word 0, Bit 0  →  PFN = 0x100000/4096 + 0   = 256   → Phys: 0x100000
      Word 0, Bit 1  →  PFN = 0x100000/4096 + 1   = 257   → Phys: 0x101000
      Word 0, Bit 63 →  PFN = 0x100000/4096 + 63  = 319   → Phys: 0x13F000
      Word 1, Bit 0  →  PFN = 0x100000/4096 + 64  = 320   → Phys: 0x140000


    ═══════════════════════════════════════════════════════════════════
                         CONCRETE MEMORY EXAMPLE
    ═══════════════════════════════════════════════════════════════════

    Assuming: kernel_size = 0x8000 (32KB), 1 memory region with 128MB

    0x00100000  ┌──────────────────────────────────┐ ◄─ KERNEL_OFFSET
                │         Kernel (32 KB)           │
    0x00108000  ├──────────────────────────────────┤ ◄─ kernel end
                │      (padding to 4KB align)      │
    0x00109000  ├──────────────────────────────────┤ ◄─ PmmLayoutHeader
                │  region_count: 1                 │    (4 bytes)
                │  padding: 0                      │    (4 bytes)
    0x00109008  │  regions[0]:                     │    (40 bytes)
                │    start: 0x100000               │
                │    frames_total: 32768           │    (128MB / 4KB)
                │    frames_free: 32768            │
                │    bitmap_start: 0x00109030 ──────────┐
                │    bitmap_bytes: 4096            │    │
    0x00109030  ├──────────────────────────────────┤◄───┘
                │                                  │
                │     Bitmap (4096 bytes)          │
                │     32768 bits = 32768 pages     │
                │                                  │
    0x0010A030  ├──────────────────────────────────┤
                │                                  │
                │     FREE MEMORY STARTS HERE      │
                │                                  │
                │    (first ~10 pages marked used  │
                │     for kernel + PMM metadata)   │
                │                                  │
    0x08000000  └──────────────────────────────────┘ ◄─ End of 128MB
    (128 MB)
```

> **The diagram above is the BIOS / fallback layout** (small RAM, metadata placed right after
> the kernel BSS). The UEFI path is different — see below.

### UEFI physical memory layout

On the UEFI path the PMM does **not** place its metadata right after the kernel. Two things move:
the kernel occupies a fixed reserved low block, and the metadata lives in a **dedicated,
loader-reserved region high in RAM** (`BootInfo.pmm_metadata_base`; see §4 and
[`uefi.md`](uefi.md) §3.3). The data *structures* (`PmmLayoutHeader`, `PmmRegion`, bitmaps) are
identical — only their physical location differs.

```text
              UEFI PHYSICAL MEMORY LAYOUT (e.g. a 128 GiB machine)
    ═══════════════════════════════════════════════════════════════════

    0x00000000 ┌────────────────────────────────────────────────────────┐
               │   Low / firmware-owned memory                          │
               │   (real-mode area, firmware data, …)                   │
    0x00100000 ├────────────────────────────────────────────────────────┤ ◄─ KERNEL_OFFSET (1 MiB)
               │   KERNEL BLOCK — 768 pages = 3 MiB, EfiLoaderCode      │
               │     • kernel image: .text/.rodata/.data/.bss           │
               │       (ends ~0x15A588, i.e. < 1.4 MiB)                 │
               │     • free space inside the block                      │
               │     • bootstrap stack — grows DOWN from 0x400000       │
    0x00400000 ├────────────────────────────────────────────────────────┤ ◄─ STACK_TOP (4 MiB)
               │                                                        │
               │   General RAM: a mix of                                │
               │     • EfiConventionalMemory  → PMM "usable" regions    │
               │     • firmware-reserved: ACPI, MMIO holes, runtime     │
               │       services, SMM/TSEG (typically near top of RAM)   │
               │                                                        │
        (high) ├────────────────────────────────────────────────────────┤ ◄─ pmm_metadata_base
               │   PMM METADATA REGION (EfiLoaderData, loader-reserved) │
               │     PmmLayoutHeader → PmmRegion[] → bitmaps            │
               │     (sized to RAM: ~32 KiB per GiB → ~4 MiB @128 GiB)  │
               └────────────────────────────────────────────────────────┘

    Elsewhere (firmware-chosen address, inside the low 512 GiB):
      • the UEFI loader image (BOOTX64.EFI), which contains:
          - the BootInfo struct  (RDI points here at kernel entry)
          - the UnifiedMemoryEntry[] array (BootInfo.memory_map_addr)
      The kernel reads both directly via the identity mapping (phys == virt).
```

Consequences specific to UEFI (all consistent with §4):

- The PMM's **usable** regions are the `EfiConventionalMemory` entries with `start >= KERNEL_OFFSET`
  (the `0x100000..0x400000` kernel block is `EfiLoaderCode`, so it is *not* usable and is never
  handed out).
- Because the metadata sits high, the single `mark_range_used(KERNEL_OFFSET, metadata_end)`
  reservation currently marks **all RAM below the metadata as used** (see §4 step 6, "Known
  limitation") — wasteful but safe.
- `BootInfo` and the `UnifiedMemoryEntry[]` array remain reachable after the kernel switches CR3
  only because the kernel's page tables keep the firmware identity map (see [`vmm.md`](vmm.md) §4).

---

## 3. Data Structures

The system's structures are annotated with `#[repr(C)]` to guarantee a stable layout matching the physical layout exactly.

### `PmmLayoutHeader`
Defines the prefix of the PMM structure located right after the kernel.
```rust
#[repr(C)]
pub struct PmmLayoutHeader {
    pub region_count: u32,
    pub padding: u32, // Guarantees 8-byte alignment for the following pointer
    pub regions_ptr: *mut PmmRegion,
}
```

### `PmmRegion`
Contains metadata for a single physical memory block.
```rust
#[repr(C)]
pub struct PmmRegion {
    pub start: u64,         // Physical start address of the region
    pub frames_total: u64,  // Total frames in the region
    pub frames_free: u64,   // Free frames remaining
    pub bitmap_start: u64,  // Physical start address of the region's bitmap
    pub bitmap_bytes: u64,  // Size of the bitmap in bytes (aligned to 8 bytes)
}
```

### `PageFrame`
A logical handle returned upon successful allocation.
```rust
pub struct PageFrame {
    pub pfn: u64,           // Page Frame Number (physical_address / PAGE_SIZE)
    pub region_index: u32,  // Internal index of the region tracking this frame
}
```

---

## 4. Initialization Workflow

### 4.0 Two memory-map sources: UEFI vs BIOS

The PMM consumes its memory map from one of two sources, chosen at runtime by whether a unified
`BootInfo` pointer was published in `KernelMain`:

- **UEFI path** (`BOOT_INFO_PTR != 0`): the map is the `UnifiedMemoryEntry[]` array the UEFI
  loader built from the firmware memory map just before `ExitBootServices`
  (`{ start, size, is_usable }`; see [`uefi.md`](uefi.md) §3.5). The metadata region is the
  loader-reserved `BootInfo.pmm_metadata_base`.
- **BIOS path / integration tests** (no `BootInfo`): the map is the legacy `BiosInformationBlock`
  (BIB) + BIOS memory map at fixed low addresses, and the metadata is placed right after the
  kernel BSS.

When the kernel initializes the PMM during `init()`, the following steps are performed:

1. **Locating Boot Data**:
   - *UEFI*: read `BootInfo.memory_map_addr` / `memory_map_len` for the `UnifiedMemoryEntry[]`
     array, and `BootInfo.pmm_metadata_base` / `pmm_metadata_size` for where to put the metadata.
   - *BIOS*: query the `BiosInformationBlock` (BIB) and BIOS Memory Map from fixed addresses
     loaded by the bootloader (`BIB_OFFSET` = `0x1000`, `MEMORYMAP_OFFSET` = `0x1200`).
2. **Placing PMM Metadata**:
   - *UEFI*: the `PmmLayoutHeader` starts at `BootInfo.pmm_metadata_base` (page-aligned). The
     loader sized this region to the machine's RAM and allocated it via
     `AllocatePages(AllocateAnyPages, …)` while boot services were alive, precisely because the
     bitmap is far too large to fit in low memory on big-RAM systems (~32 KiB per GiB → ~4 MiB at
     128 GiB).
   - *BIOS / fallback*: the end of the kernel virtual address space (`__bss_end`) is converted to
     a physical address via `virt_to_phys()` and aligned up to 4 KiB; that defines the start of
     the `PmmLayoutHeader`.
3. **Usable Memory Filtering**: The memory map entries are parsed. A memory region is classified as usable if:
   - Its type is usable (`region_type == 1`).
   - Its base physical address starts at or above `KERNEL_OFFSET` (`0x100000`, 1 MiB).
4. **Header and Region Initialization**: 
   - `region_count` is written to the header.
   - Usable regions are populated in the sequential array of `PmmRegion` structs starting at `regions_ptr`.
5. **Bitmap Placement**: Bitmaps are mapped sequentially right after the `PmmRegion` array. The PMM writes zeroes across all bitmap pages to mark all frames as free by default.
6. **Kernel & Metadata Reservation**: To prevent the allocator from overwriting vital operating system data, `mark_range_used()` is executed on the physical range from `KERNEL_OFFSET` (`0x100000`) to `reserved_end` — the maximum of the bootloader stack limit `STACK_TOP` (`0x400000`) and the end of the PMM bitmaps, aligned up to 4 KiB. This marks the kernel, bootloader stack, and PMM structures themselves as allocated.

> **⚠ Known limitation (open follow-up) on the UEFI path.** `reserved_end` is computed as a
> *single contiguous span* `[KERNEL_OFFSET, max(metadata_end, STACK_TOP))`. On the BIOS path the
> metadata sits just past the kernel, so this span is small and correct. On the UEFI path,
> however, `metadata_end` lies at `pmm_metadata_base + bitmaps`, which the firmware may place tens
> of GiB up — so this one call marks **all RAM from 1 MiB up to the metadata region as used**,
> wasting most of it and forcing the first real allocations (e.g. the kernel PML4 frame in
> `vmm::init`) to come from *above* the metadata. It is *safe* (nothing is corrupted) but
> *wasteful*. The correct fix is to reserve **two separate ranges** — the low kernel+stack block
> and the metadata region — instead of one giant span. Note: the high placement of the first
> allocations was investigated and proven **not** to be the cause of the historical UEFI CR3-load
> reset (see [`vmm.md`](vmm.md) §4.3); it is purely a memory-waste issue.

---

## 5. Allocation & Release Algorithms

### Frame Allocation (`alloc_frame`)
The PMM searches through memory regions in linear order:
1. Skips regions with `frames_free == 0`.
2. Scans the bitmap in 64-bit words (`u64`).
3. If a word is not `u64::MAX`, it has at least one free bit (set to `0`).
4. Identifies the free bit using `(!val).trailing_zeros()`.
5. Calculates the absolute bit index within the region.
6. If the bit index is within `frames_total`, it sets the bit to `1` using `set_bit()`, decrements `frames_free`, and returns the `PageFrame`.

### Frame Release (`release_pfn`)
To release a physical page frame given its PFN:
1. Iterates over the regions to locate the region where `pfn` falls inside `[region_start_pfn, region_end_pfn)`.
2. Computes the offset `bit_idx = pfn - region_start_pfn`.
3. Checks if the bit is currently set (`1`). If it is already `0` (free), returns `false` (error/double-free protection).
4. Clears the bit using `clear_bit()`, increments `frames_free`, and returns `true`.

---

## 6. Bitwise Operations

Since bits map directly to physical frames within each region, the PMM reads and writes the physical bitmap in terms of `u64` blocks:
- **Set Bit**:
  ```rust
  let word = base.add((idx / 64) as usize);
  let mask = 1u64 << (idx % 64);
  *word |= mask;
  ```
- **Clear Bit**:
  ```rust
  let word = base.add((idx / 64) as usize);
  let mask = 1u64 << (idx % 64);
  *word &= !mask;
  ```
