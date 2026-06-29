# Implementation Plan: Kernel-Owned Page Tables on the UEFI Path

> **Audience:** Coding AI, for step-by-step implementation.
> **Status:** Plan, not yet implemented.
> **Predecessor context:** `docs/vmm.md` §4 (write_cr3 saga), `docs/boot_uefi.md`.

---

## 1. Motivation & Problem Statement

On the UEFI boot path the kernel today **inherits** the firmware's page tables:

1. The loader (`kaosldr_uefi/src/main.rs:756-782`) only mirrors PML4 entry 0 → 256 to
   make the higher-half kernel visible. The hierarchy below it (PDPT/PD/PT, full of
   **huge pages**) stays firmware-owned.
2. `vmm::init` (`kernel/src/memory/vmm/mod.rs:275-301`) does build a new PML4 root, but
   only as a **shallow superset**: `build_kernel_pml4_from_firmware`
   (`page_table.rs:416-431`) copies the 512 top-level entries verbatim and only installs
   the recursive self-map in slot 511. The PML4 entries keep pointing at firmware-owned
   sub-tables.

This yields five structural problems:

| # | Problem | Current code site |
|---|---------|-------------------|
| P1 | **No W^X**: kernel text runs RWX (firmware maps identity as supervisor-RWX, huge pages cannot be split) | inherited map in slot 0/256 |
| P2 | **Direct map depends on firmware coverage**: `virt_to_phys` is a pure offset (`pmm/types.rs:23`); unmapped RAM → silent `#PF` | `pmm/types.rs:13,23` |
| P3 | **Firmware PT frames permanently blocked + fragile reservation** | `vmm::reserve_firmware_page_tables` (`main.rs:156`, `page_table.rs:455`) |
| P4 | **Caching/MMIO inherited blindly**: no per-page override possible (no split) | huge pages in slot 0/256 |
| P5 | **Two divergent memory models** (legacy builds its own, UEFI inherits) | entire VMM |

**Goal:** Early in `KernelMain`, the kernel builds its **own, complete**
page-table hierarchy from the UEFI memory map, switches CR3 to it, and frees the
firmware sub-tables. This resolves P1–P5.

---

## 2. Verified Code Facts (Starting Point)

These facts were checked against the current code — the implementation must respect them:

- **`UnifiedMemoryEntry` today carries only `{ start: u64, size: u64, is_usable: bool }`**
  — duplicated identically in `kernel/src/boot_info.rs:68-80` **and**
  `kaosldr_uefi/src/main.rs:343-349`. Both `#[repr(C)]`, must stay layout-identical.
- **The loader collapses every descriptor to `is_usable = (memory_type == 7)`**
  (`kaosldr_uefi/src/main.rs:740`). The `EfiMemoryDescriptor` (`main.rs:199-210`) already
  has `memory_type: u32` and `attribute: u64` — they are simply not forwarded.
- **The walker rejects huge pages**: `pt_for_if_present` (`page_table.rs:530`) and the
  reservation walks (`page_table.rs:476-487`) bail on `pde.huge()`/`pdpte.huge()`.
- **The VMM today creates exclusively 4 KiB mappings** — there is deliberately no
  `set_huge` setter (`page_table.rs:193-194`). Huge-page creation must be built.
- **Existing bit constants** (`page_table.rs:11-26`): `ENTRY_PRESENT`,
  `ENTRY_WRITABLE`, `ENTRY_USER`, `ENTRY_PWT`, `ENTRY_PCD`, `ENTRY_HUGE` (`1 << 7`),
  `ENTRY_GLOBAL`, `ENTRY_NO_EXECUTE`, `ENTRY_FRAME_MASK`.
- **Entry setters present** (`page_table.rs`): `set_present`, `set_writable`,
  `set_user`, `set_no_execute`, `set_mapping(pfn, present, writable, user)`, `set_frame`.
- **EFER.NXE is already active** — `arch::msr::enable_no_execute()` in `main.rs:142`.
- **Current boot order in `KernelMain`** (`kernel/src/main.rs`):
  ```
  142  arch::msr::enable_no_execute()
  146  pmm::init(true)
  156  vmm::reserve_firmware_page_tables()   // unsafe, conditional
  161  interrupts::init()
  166  vmm::init(true)                        // builds superset PML4, write_cr3
  170  heap::init(true)
  ```
- **`vmm::init`** allocates a PML4 frame, calls `build_kernel_pml4_from_firmware`,
  `set_vmm_state_unchecked` (`mod.rs:189`), `write_cr3` (`mod.rs:300`).
- **Test conventions**: integration tests in `kernel/tests/`. Relevant:
  `boot_info_layout_test.rs`, `page_table_test.rs`, `pmm_uefi_test.rs`, `vmm_test.rs`,
  `pmm_metadata_base_test.rs`.

---

## 3. What Must Be (Re-)Mapped After the CR3 Switch

Audit of all post-CR3 dependencies on the identity/firmware map. The kernel-owned
tables **must** cover the following before the switch:

1. **All RAM the PMM hands out** — `zero_phys_page`, page-table-frame writes, and every
   `alloc_frame()` consumer dereferences physical addresses via the identity map.
2. **PMM metadata region** — header/regions/bitmaps are written by physical address
   (`pmm/manager.rs`); on UEFI it may sit tens of GiB up
   (`pmm_metadata_base`, set by the loader at `main.rs:689`).
3. **BootInfo + memory-map array** — read by physical address
   (`memory_map_addr`, `pmm/manager.rs`). Lives in loader memory (`EfiLoaderData`).
4. **GOP framebuffer MMIO** — `fb_info.base_address` is written directly
   (`main.rs` gradient/heartbeat).
5. **Firmware regions the platform/SMM needs** — every entry with
   `EFI_MEMORY_RUNTIME` (`0x8000_0000_0000_0000`), plus
   `RuntimeServicesCode/Data`, `ACPIMemoryNVS`, `Reserved`, `MemoryMappedIO`, `PalCode`.
6. **Higher-half kernel (PML4[256]) + recursive window (PML4[511])** — already
   kernel-owned, must be preserved.

**Safe to drop** (no longer referenced after the switch):
firmware-owned PDPT/PD/PT frames; `BootServicesCode/Data`, `LoaderCode`, unused
`ConventionalMemory`. **But:** BootInfo, the memory map, and the kernel image live in
loader memory → handle those regions explicitly, do not free them wholesale.

---

## 4. EFI Memory Type Reference (for Loader & Classification)

```
0  EfiReservedMemoryType      -> map (Reserved)
1  EfiLoaderCode              -> BootInfo/map may be here; otherwise drop
2  EfiLoaderData              -> BootInfo/map/PMM-meta live here; keep explicitly
3  EfiBootServicesCode        -> drop
4  EfiBootServicesData        -> drop
5  EfiRuntimeServicesCode     -> map
6  EfiRuntimeServicesData     -> map
7  EfiConventionalMemory      -> RAM (direct map), mark usable
8  EfiUnusableMemory          -> do not map
9  EfiACPIReclaimMemory       -> RAM after ACPI parse; map for now, not usable
10 EfiACPIMemoryNVS           -> map
11 EfiMemoryMappedIO          -> map (NX, uncacheable)
12 EfiMemoryMappedIOPortSpace -> map
13 EfiPalCode                 -> map
14 EfiPersistentMemory        -> as needed
Attribute bit: EFI_MEMORY_RUNTIME = 0x8000_0000_0000_0000  -> always map
```

---

## 5. Implementation Phases

Each phase is independently buildable/testable. The order is **binding** (Phase 0 is a
hard prerequisite). After each phase: `cargo build` + `cargo test` from `main64/` must be
green; the QEMU boot must not regress.

---

### Phase 0 — Loader forwards EFI type + attribute *(hard prerequisite)*

**Why first:** Without type/attribute the kernel cannot distinguish runtime/reserved/NVS
from "plain reserved" → Phase 2 is impossible.

**Changes:**
1. Extend `UnifiedMemoryEntry` in **both** definitions
   (`kernel/src/boot_info.rs:68-80` **and** `kaosldr_uefi/src/main.rs:343-349`) — keep
   the layout exactly identical:
   ```rust
   #[repr(C)]
   pub struct UnifiedMemoryEntry {
       pub start: u64,
       pub size: u64,
       pub memory_type: u32,   // NEW: EFI descriptor type (0..=14)
       pub _pad: u32,          // NEW: explicit padding for u64 alignment of the next field
       pub attribute: u64,     // NEW: EFI descriptor attribute (incl. EFI_MEMORY_RUNTIME)
       pub is_usable: bool,    // kept: derived convenience
       // mind repr(C) tail padding to 8
   }
   ```
   > Note: choose the field order so `#[repr(C)]` is identical on both sides. Make `_pad`
   > explicit so the layout test stays stable.
2. Adjust loader population (`kaosldr_uefi/src/main.rs:734-750`): carry over
   `memory_type` and `attribute` from the `EfiMemoryDescriptor` (`main.rs:199-210`);
   derive `is_usable` as before (`memory_type == 7`), but now from the raw data.
3. Update `kernel/tests/boot_info_layout_test.rs` for the new offsets/size.
4. Extend the loader's `static UNIFIED_MEM_MAP` initializer (`main.rs:366-370`) with the
   new fields.

**Acceptance:** Both crates build green; `boot_info_layout_test` green; QEMU boot
unchanged; `memory_type`/`attribute` are readable in the kernel (a short debug dump in
`KernelMain` for visual inspection, then removed).

---

### Phase 1 — Kernel-built direct map of *all* RAM

**Goal:** A kernel-owned PML4/PDPT/PD hierarchy covering every RAM region — not just the
inherited low regions. For now **identity at PML4[0]** (minimizes churn, since PMM/frame
code assumes identity). A higher-half `PAGE_OFFSET` direct map (freeing PML4[0] for user
space) is a later option.

**Changes:**
1. New module, e.g. `kernel/src/memory/vmm/direct_map.rs`, with a **pure, testable**
   table-builder function:
   - Input: iterator over `UnifiedMemoryEntry` + a PMM frame-allocator callback.
   - Output: physical address of the new PML4 + list of allocated PT frames.
   - Logic: for each RAM region (`memory_type == 7`, plus the kept types listed in §3)
     enter the VA=PA mappings into the PML4[0] subtree.
2. **Huge-page creation** (sub-task, see Phase 1a): 2 MiB pages for the bulk map.
   At 128 GiB, 4 KiB tables would cost ~256 MiB of page tables.
3. **Build in coverage validation** (recommended, pulled forward from Phase 6): after the
   build, verify that every `is_usable` region resolves fully in the new map; otherwise
   **panic loudly** — this catches a Phase 1 error as a clear panic instead of later
   misreading it as an SMM reset (Phase 4).
4. **No** CR3 switch yet in this phase — only build + validate, the old superset PML4
   stays active.

**Acceptance:** Unit test of the builder (frame math + region classification) in the
style of `page_table_test.rs`/`pmm_uefi_test.rs`; coverage validation passes at boot
without panic (QEMU + HW if possible).

#### Phase 1a — Huge-page support in the VMM *(prerequisite for 1)*
- Add a `set_huge` path: 2 MiB PD-leaf creation (`ENTRY_HUGE`, `page_table.rs:16`).
- The walker (`pt_for_if_present` `page_table.rs:530`, reservation walks `476-487`) must
  still recognize huge leaves correctly (they already do: `huge()`), but add new helpers
  for "resolve VA through a huge page" for the coverage validation.
- Alternative for a first cut: **accept the 4 KiB cost** (small RAM / QEMU only), make
  huge pages a follow-up task. Then 1a is optional for the first working version.

---

### Phase 2 — Map firmware/platform regions explicitly

**Goal:** Using the type/attribute from Phase 0, map every region the platform/SMM needs
(§3.5): `EFI_MEMORY_RUNTIME` bit set, plus `RuntimeServicesCode/Data` (5/6),
`ACPIMemoryNVS` (10), `Reserved` (0), `MemoryMappedIO` (11), `PalCode` (13).

**Note:** KAOS calls **no** runtime services today → no `SetVirtualAddressMap` /
`efi_switch_mm` needed. Simply keep these regions mapped in the kernel tables.
Document Linux-grade isolation (separate `efi_mm`) as a future enhancement.

**Acceptance:** Classification unit tests (which type/which attribute → map?); the kept
region list is logged (for later bisecting).

---

### Phase 3 — Map the GOP framebuffer explicitly

**Goal:** Map `[fb_info.base_address, +fb_info.size)` (`boot_info.rs:44-49`) in the
kernel tables, ideally **write-combining** (PAT/`ENTRY_PWT`/`ENTRY_PCD`), NX. Today this
relies on the inherited firmware map.

**Acceptance:** After the CR3 switch (Phase 4) the framebuffer stays writable (gradient/
heartbeat in `main.rs` visible).

---

### Phase 4 — Drop firmware sub-tables + CR3 switch

**Goal:** Once the kernel tables cover all RAM (P1) + the firmware/runtime/MMIO set (P2) +
framebuffer (P3) + slot 511 + slot 256: `write_cr3` to the kernel-owned PML4.

**Changes:**
1. Rework the `KernelMain` order (`main.rs:142-170`): the new direct-map build goes
   **before** the final `write_cr3`. Possibly rework `vmm::init` so it installs the new
   complete map instead of the superset.
2. Firmware PDPT/PD/PT frames are no longer referenced →
   **remove** `reserve_firmware_page_tables()` (`main.rs:156`, `page_table.rs:455`);
   those frames return to the PMM (memory win, resolves P3).
3. **Keep a fallback:** retain the old full-clone path (`build_kernel_pml4_from_firmware`)
   behind a `const`/build flag (e.g. `cfg!(feature = "uefi_full_clone")`) as a fallback,
   **until validated on real HW**.

**Acceptance:** QEMU boot to the ring-3 shell green; on HW see Risks (§6).
This is the point with the highest reset risk (first time away from the firmware map again
— cf. `docs/vmm.md` §4).

---

### Phase 5 — Permission hardening (W^X, resolves P1)

**Goal:** W^X across the kernel-owned map:
- Kernel code RO+X.
- Kernel data + direct map: NX (`set_no_execute(true)`).
- Framebuffer/MMIO: NX.

EFER.NXE is already active (`main.rs:142`). Derive the kernel `.text`/`.rodata` bounds
from the linker script / `kernel_size`; use 4 KiB granularity only for the kernel-image
region if needed (the rest of the direct map may stay huge).

**Acceptance:** A write attempt to kernel `.text` → `#PF` (targeted death test in the
style of `page_fault_death_test.rs`); boot stays green.

---

### Phase 6 — Validation

1. **Unit-test** the pure table builder (map-all-RAM frame math + region classification)
   in the style of `page_table_test.rs` / `pmm_uefi_test.rs`.
2. **Real hardware is mandatory** and the only real test: run the `docs/boot_uefi.md`
   smoke-test checklist on the AMD/UEFI box **after Phase 4** and again **after Phase 5**.
3. Account for an SMM-class reset → be ready to bisect the kept region set.

---

## 6. Risks

| Risk | Mitigation |
|------|------------|
| **HW-only validatable** — QEMU tolerates any map, only the AMD box exercises the SMM path | Fallback flag (Phase 4); log region set for bisecting |
| **Platform-specific firmware set** — an SMI may touch something outside RUNTIME+Reserved/NVS → reset | Widen the kept set or fall back to the full clone |
| **Huge-page support is a real prerequisite** for a cheap large-RAM direct map | Phase 1a; or accept 4 KiB cost initially (small RAM / QEMU only) |
| **Layout drift** of the duplicated `UnifiedMemoryEntry` | `boot_info_layout_test` as guard; change both defs in one commit |
| **Coverage gap** in Phase 1 only surfaces as a reset in Phase 4 | Coverage validation with a loud panic already in Phase 1 |

---

## 7. Definition of Done

- [ ] Phases 0–6 implemented, each green (`cargo build` + `cargo test` from `main64/`).
- [ ] UEFI boot to the ring-3 shell on QEMU **and** on the AMD/UEFI HW.
- [ ] `reserve_firmware_page_tables` removed; firmware PT frames back in the PMM.
- [ ] Kernel `.text` is RO+X; data/direct-map/MMIO are NX (W^X verified).
- [ ] Fallback path (full clone) documented behind a flag and disablable once HW-validated.
- [ ] `docs/vmm.md` extended with the new model; this plan marked "implemented".
