# Implementation Plan: Kernel-Owned Page Tables on the UEFI Path

> **Audience:** Coding AI, for step-by-step implementation.
> **Status:** Phases 0-4 implemented and now the **unconditional standard path**: on
> every boot that publishes a `BootInfo` (i.e. every real boot), `vmm::init` builds a
> kernel-owned page-table hierarchy and switches CR3 to it. Phase 5 partial (NX done,
> kernel `.text` RO+X not yet done — see `kernel/src/memory/vmm/direct_map.rs`'s
> `build_full_kernel_pml4` doc comment). Tracked in issue #63, branch
> `feature/issue-63-uefi-kernel-pagetables`.
> The kernel-owned table is validated end-to-end: QEMU/OVMF **and** the real AMD/UEFI
> box boot to the ring-3 shell (all shell commands + `TUI.BIN`), and `cargo test` is
> green. The firmware-clone path is kept only as the fallback for the no-`BootInfo`
> case (`vmm::init` gates the switch on a published `BootInfo`).
>
> **Post-validation cleanup (2026-07-29):** now that the real-hardware boot is proven,
> the `USE_DIRECT_MAP_TABLE` const kill-switch/rollout flag was removed (the switch is
> no longer gated on a compile-time flag, only on `BootInfo` presence), and the
> redundant Phase 1 boot canary (`run_boot_canary` + its `free_direct_map_tables`
> helper — a build+validate+free pass that duplicated the coverage validation
> `build_full_kernel_pml4` already performs before the CR3 switch) was removed. The
> historical mentions of the flag/canary in §5 below are kept for context.
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

### Implementation status (2026-07-27, issue #63)

All phases below are implemented on `feature/issue-63-uefi-kernel-pagetables`. Notable
deviations from the plan as originally written:

- **A third `UnifiedMemoryEntry` copy** (`kaosldr_64/src/boot_info.rs`, the BIOS loader)
  was missing from this document's Phase 0 scope. The kernel reads the memory map
  boot-path-agnostically, so leaving it at the old 24-byte layout would have silently
  broken the BIOS boot path the moment the other two copies grew to 40 bytes. Fixed in
  the same commit as the UEFI/kernel struct changes.
- **Real file paths differ slightly** from this doc's references (mostly line-number
  drift, plus `kernel/src/memory/pmm/types.rs`, not `kernel/src/pmm/types.rs`).
- **A real bug was caught by `direct_map_full_switch_test.rs`** (a QEMU-only test that
  calls the actual CR3-switch path directly): `build_direct_map` did not round each
  memory-map region to page boundaries before mapping. QEMU/SeaBIOS's classic
  `[0x0, 0x9FC00)` usable / `[0x9FC00, 0xA0000)` reserved split — both inside the same
  4 KiB page — made `phys_to_pfn`'s implicit `>>12` truncate an unaligned region start,
  which then mismatched against the untruncated address in the idempotency check and
  raised a spurious `Overlap` error. Fixed by rounding each region outward to page
  boundaries in `build_direct_map`; regression-tested in `direct_map_test.rs`.
- **Phase 4's CR3 switch is implemented and ENABLED** (`direct_map::USE_DIRECT_MAP_TABLE
  = true` since 2026-07-28) and validated on the real AMD/UEFI box (boots to the ring-3
  shell; all shell commands and `TUI.BIN` work). QEMU cannot reproduce the SMM/SMI
  regression class that motivated the firmware-clone approach (see §6), so the
  real-hardware boot — not the QEMU pass — is what cleared it. `vmm::init` gates the
  switch on a published `BootInfo`, falling back to the firmware clone otherwise (e.g.
  unit-test kernels). (The `USE_DIRECT_MAP_TABLE` const kill-switch that guarded this at
  the time was removed on 2026-07-29 once the real-hardware boot was proven — see the
  post-validation cleanup note at the top of this document.)
- **Phase 5 is partial**: NX across the whole kernel-owned identity/direct map is done;
  kernel `.text` RO+X is not (it requires page-aligning `link.ld` and rebuilding the
  higher-half PML4 slot at 4 KiB granularity — a higher-blast-radius change than the
  rest of this effort, left as explicit follow-up).

### Activation-path review follow-up (2026-07-28)

A review of `USE_DIRECT_MAP_TABLE`'s activation path (run while it was still disabled,
before the flip described in the next subsection) found five gaps, all now fixed:

1. **`EfiLoaderCode`/`EfiLoaderData` (types 1/2) were unmapped** by either classifier —
   on UEFI, `kaosldr_uefi` allocates the PMM-metadata region as `EfiLoaderData`
   (`allocate_pages(0, 2, …)`), and the loader's own `BootInfo`/memory-map statics
   typically live in one of these two types too. Fixed with a new `is_loader_owned`
   classifier, wired as a third `build_direct_map` pass.
2. **No independent sanity check** that the addresses the kernel actually dereferences
   (`BootInfo` itself, its memory map, the PMM-metadata region) resolve, regardless of
   classifier coverage. Fixed with `validate_essential_boot_addresses`, deliberately
   redundant with the classifiers so a future classifier regression can't silently
   reopen this exact gap.
3. **No test exercised the UEFI-shaped memory layout** — `direct_map_full_switch_test.rs`
   only ever boots via the BIOS loader, where the PMM-metadata region falls back to
   plain usable RAM. Fixed with a synthetic-memory-map test
   (`test_build_full_kernel_pml4_maps_uefi_style_loader_data_metadata`) that calls
   `build_full_kernel_pml4` directly with an `EfiLoaderData`-typed metadata region,
   without a real CR3 switch (an incomplete synthetic map doing a real switch could
   crash the test kernel). A full UEFI-boot (OVMF/GPT) test harness remains a separate,
   larger follow-up, not blocking for this issue.
4. **The framebuffer was never actually mapped** by the switch path — `map_wc_range`
   was implemented and unit-tested but the doc comment describing it as "the caller's
   responsibility" was never acted on by the one production caller. Fixed:
   `build_full_kernel_pml4` now calls it when a framebuffer is present.
5. **MMIO (`EfiMemoryMappedIO`, 11) was mapped write-back**, not uncacheable — it went
   through the same `is_phase2_platform`/`map_2m_page`/`map_4k_page` path as
   Reserved/RuntimeServices/ACPI-NVS/PalCode, which only sets NX. Fixed by splitting
   MMIO into its own `is_mmio` classifier mapped via a new `build_uc_direct_map`
   (PCD set, PWT clear, 4 KiB-only, mirroring `map_wc_range`'s reasoning).

6. **`EfiMemoryMappedIOPortSpace` (type 12) was classified nowhere** — not
   `is_phase1_ram`, not `is_phase2_platform`, not `is_mmio` — so a region of this type
   was silently left unmapped by every pass, rather than mapped as §4 requires. Found in
   a follow-up review after the five points above. Fixed by widening `is_mmio` to accept
   both 11 and 12: architecturally the same class of device-backed address-space window
   as `EfiMemoryMappedIO`, so it goes through the same uncacheable `build_uc_direct_map`
   path rather than getting a fourth classifier/pass.

While wiring the point-3 test's cleanup, also found and fixed a latent bug in
`free_direct_map_tables`: it walked every PML4 slot unconditionally, including slot 256
(the higher-half mirror, borrowed verbatim from whatever table was active when
`build_full_kernel_pml4` ran — freeing it would release frames a different, still-live
table depends on) and slot 511 (the recursive self-map, misinterpreted as a regular
PDPT entry one level down, plus a double-release of the PML4's own frame). Never
triggered before because `free_direct_map_tables` had only ever been called on plain
`build_direct_map`-only canary tables, which never populate either slot.

### Activation on real hardware (2026-07-28)

With the five gaps above fixed, `USE_DIRECT_MAP_TABLE` was flipped to `true` and the
branch was validated on the physical AMD/UEFI box: it boots to the ring-3 shell and all
shell commands work. Two further issues surfaced **only on real hardware** (both fixed):

1. **`TUI.BIN` crashed at startup** — `page_fault.rs` "protection page fault at 0x100e".
   The `GetBiosMemoryMapEntryCount`/`GetBiosMemoryMapEntry` syscalls read the BIOS
   Information Block at the fixed low physical address `BIB_OFFSET` (0x1000; `0x100e` =
   `+ offset_of(memory_map_entries)`), a BIOS-only structure. Harmless under the firmware
   clone (the firmware identity-maps 0x1000, so the read returns garbage), but the
   kernel-owned table does **not** map that low page on real UEFI firmware (it is a
   dropped type there), so the read faulted. QEMU/OVMF maps 0x1000, so it reproduced only
   on hardware; TUI-specific because only TUI issues that syscall at startup. Fixed: both
   syscalls now read the loader's `UnifiedMemoryEntry` map when a `BootInfo` is present
   (mirroring `drivers::time`'s guard), touching 0x1000 only on the legacy no-`BootInfo`
   path. **General lesson:** kernel-owned tables expose *any* latent read of a hardcoded
   low physical address that the firmware identity map used to satisfy silently — audit
   for other such derefs.
2. **`cargo test` went red** — ~20 test kernels call `vmm::init` without publishing a
   `BootInfo`, tripping `switch_to_direct_map`'s BootInfo assertion. Fixed by gating the
   switch on `USE_DIRECT_MAP_TABLE && BOOT_INFO_PTR != 0`; with no `BootInfo`, `vmm::init`
   falls back to the firmware clone (identical to the `false` behavior). `cargo test` is
   green again (401/401 across 39 test files).

Branch commits: `57a2ce7` (MMIO/phase2 RUNTIME split), `e82329a` (enable flag + visible
boot banner), `9c1d8d9` (BIOS-syscall fix), `5794b5c` (BootInfo-gate).

### R1: firmware-table / PMM-pool disjointness invariant (2026-07-29)

Skipping `reserve_firmware_page_tables()` on the kernel-owned-table path rests on a
load-bearing invariant that was previously only implicit (the module doc argued that
scaffold frames are *reachable*, but not that they can never *alias* a live table frame):

> **Invariant.** No frame of the currently-active firmware/BIOS-loader page tables is
> ever a frame the PMM can allocate.

Why it matters: `switch_to_direct_map` draws scaffold frames from the PMM *while the
firmware/loader tables are still live in CR3*, and `build_full_kernel_pml4` copies the
higher-half mirror (PML4 slot 256) verbatim, so a firmware sub-tree stays referenced
*after* the switch too. If the PMM could hand out one of those live frames, zeroing it
during the build — or reusing it at runtime — would corrupt address translation and
hard-reset the machine with no diagnostic (a real-hardware-only failure).

Why it holds by construction: the PMM pools **only** usable RAM at or above
`KERNEL_OFFSET` (1 MiB), whereas the active tables live outside that pool — on UEFI in
firmware-owned, non-`EfiConventionalMemory` memory; on BIOS in the loader's
`0x9000..=0x15FFF` tables, all below 1 MiB.

What was done for R1:
- **Documented** the invariant (both facets) at the reserve-skip site in `main.rs` and in
  `direct_map.rs`'s module doc.
- **Guarded** it: `switch_to_direct_map` now calls
  `page_table::assert_no_active_table_frame_is_pmm_free(old_pml4_phys)` before drawing
  any scaffold frame. It walks the active tree and panics loudly (naming the invariant)
  if any table frame is a free PMM frame, turning a future regression (a loader that
  parks tables in usable RAM, or a PMM that pools more memory types) into a located
  panic instead of a mystery reset. Backed by a new read-only PMM query
  `PhysicalMemoryManager::is_pfn_free`.
- **Tested**: `is_pfn_free` behaviour and the guard's no-false-positive path in
  `pmm_test.rs`; the guard's panic-on-violation path in
  `firmware_tables_pmm_pool_death_test.rs`.

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
