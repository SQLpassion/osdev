# UEFI Bring-up — Outstanding Work (Handoff)

A self-contained starting point for continuing the UEFI boot work in a fresh session. What remains
is the larger **address-space rework (A)**, plus two smaller cleanup items (**B**, **C**).

## Background you need first (read these)

- `docs/uefi.md` §3 — the full UEFI boot + hand-off + kernel-init walkthrough, and the new
  real-HW smoke-test checklist.
- `docs/vmm.md` §4 — why `vmm::init` clones the firmware PML4 (the SMM/SMI reset lesson) and
  §4.4 (the open follow-up this rework addresses).
- `docs/pmm.md` §2 (UEFI layout) and §4 — where PMM metadata lives on the UEFI path.

**Current baseline:** `vmm::init` builds the kernel PML4 as a **superset of the firmware PML4**
(clone all 512 entries, add a recursive self-map at slot 511 — `build_kernel_pml4_from_firmware`).
The kernel boots on real AMD/UEFI hardware through the CR3 switch + heap + PCI + timer init, then
(on a GOP boot) stops in a black/white framebuffer heartbeat. Key files:

- `kernel/src/memory/vmm/page_table.rs` — `build_kernel_pml4_from_firmware`, `RECURSIVE_SLOT = 511`,
  `reserve_firmware_page_tables`.
- `kernel/src/memory/vmm/mod.rs` — `vmm::init`.
- `kernel/src/memory/pmm/manager.rs` — metadata-base selection + the two-range reservation.

Constraint to remember: physical RAM (incl. the cloned firmware tables, BootInfo, and the
PMM-metadata region) stays reachable after the switch only because the kernel PML4 keeps the
firmware **identity map** (`PML4[0]`). The rework below replaces *how* that reachability is
achieved (kernel-built map instead of inherited firmware map) — it must not lose it.

---

## A. Trim the kernel address space — the Linux model

**Goal:** stop inheriting the firmware's *entire* PML4 forever. Build **kernel-owned** page tables
that map only what is actually needed, then drop the firmware sub-tables (which `reserve_firmware_page_tables`
currently has to pin as "used"). This is the long-term clean design hinted at in `vmm.md` §4.4 and
the old item "4a".

### Why the Linux model (research summary)

Linux **never clones the firmware page tables**. It solves the two sub-problems separately:

1. **A full kernel-built direct map of *all* physical RAM** (the *physmap* at `PAGE_OFFSET`,
   `phys + PAGE_OFFSET = virt`, 2 MiB huge pages). Every physical RAM address is reachable in the
   active CR3 — so anything an asynchronous SMI/firmware path touches via the interrupted context
   stays valid.
2. **UEFI runtime regions mapped explicitly and in isolation:** Linux calls `SetVirtualAddressMap()`
   and keeps the `EFI_MEMORY_RUNTIME` regions in a *dedicated* page table (`efi_mm`/`efi_pgd`),
   switching to it (`efi_switch_mm()`) only around runtime-service calls.
3. **SMM is left alone** — Linux does not map SMRAM; SMM is self-contained (its own CR3 in SMRAM).

The crucial lesson for KAOS: the minimal-map reset (`vmm.md` §4.3) was **not** caused by "dropping
the firmware tables" — it was caused by **dropping all RAM above 4 MiB**. Linux drops the firmware
*tables* but maps *all RAM* + the runtime/reserved regions. "Trim" therefore means **rebuild
kernel-owned tables that re-create everything needed**, not "delete entries from the cloned map".

Sources: [LWN: EFI page table isolation](https://lwn.net/Articles/664246/) ·
[x86/efi: Build our own page table structures](https://lore.kernel.org/lkml/20171204155942.482436478@linuxfoundation.org/) ·
[x86/efi: Use efi_switch_mm()](https://www.spinics.net/lists/linux-efi/msg13293.html) ·
[Linux MM: Page Tables / PAGE_OFFSET direct map](https://docs.kernel.org/mm/page_tables.html) ·
[UEFI 2.10 §8 Runtime Services / SetVirtualAddressMap](https://uefi.org/specs/UEFI/2.10/08_Services_Runtime_Services.html).

### What the kernel demonstrably needs after the switch (must be re-created)

From a code audit of every post-CR3 identity/firmware dependency:

- **All RAM the PMM hands out** — `zero_phys_page`, page-table-frame writes, every `alloc_frame()`
  consumer dereferences physical addresses via the identity map.
- **PMM metadata region** — header/regions/bitmaps are written by physical address
  (`pmm/manager.rs`); on UEFI this sits tens of GiB up.
- **BootInfo + the memory-map array** — read by physical address (`main.rs`, `pmm/manager.rs`).
- **The GOP framebuffer MMIO** — `fb_info.base_address` written directly (`main.rs` gradient +
  heartbeat).
- **Firmware regions the platform/SMM needs** — every entry with `EFI_MEMORY_RUNTIME`, plus
  `ACPIMemoryNVS`, `Reserved`, `MemoryMappedIO`, `PalCode`.
- Higher-half kernel (`PML4[256]`) and the recursive window (`PML4[511]`) — already kernel-owned.

Safe to drop: the firmware's PDPT/PD/PT **frames** themselves (once we no longer point at them),
and `BootServices*` / `Loader*` / plain unused `ConventionalMemory` mappings — **but** BootInfo, the
memory map and the kernel image live in loader memory, so handle those regions explicitly.

### Implementation phases

**Phase 0 — Loader forwards EFI memory types + attributes (hard prerequisite).**
Today the loader collapses every descriptor to `is_usable = (memory_type == 7)`
(`kaosldr_uefi/src/main.rs:578`) and the kernel's `UnifiedMemoryEntry` only carries
`{ start, size, is_usable }`. The kernel therefore **cannot tell** runtime/reserved/ACPI-NVS from
plain reserved. Extend `UnifiedMemoryEntry` (kernel `boot_info.rs` **and** loader) with
`memory_type: u32` and `attribute: u64` (the EFI descriptor's `Type` and `Attribute`, incl.
`EFI_MEMORY_RUNTIME = 0x8000000000000000`). Keep `is_usable` as a derived convenience. Update
`tests/boot_info_layout_test.rs` for the new offsets/size. *Nothing else in A is possible without this.*

**Phase 1 — Kernel-built direct map of all RAM.**
Allocate kernel-owned PML4/PDPT/PD frames from the PMM and map **every RAM region** (not just the
low 4 MiB). Start with identity at `PML4[0]` (minimises churn — the PMM/frame code already assumes
identity); a Linux-style higher-half `PAGE_OFFSET` direct map (freeing `PML4[0]` for user space) is
a later option. Use **2 MiB huge pages** for the bulk map — at 128 GiB, 4 KiB tables would cost
~256 MiB of page tables. Note: the recursive walker rejects huge pages (`pt_for_if_present` bails on
`huge()`), and the VMM only *creates* 4 KiB mappings today, so **huge-page creation support is its
own sub-task** (or accept the 4 KiB cost initially).

**Phase 2 — Map the firmware regions the platform needs.**
Using the Phase 0 type/attribute info, map every entry with `EFI_MEMORY_RUNTIME` set, plus
`RuntimeServicesCode/Data`, `ACPIMemoryNVS`, `Reserved`, `MemoryMappedIO`, `PalCode`. KAOS calls no
runtime services today, so a dedicated `efi_mm` + `SetVirtualAddressMap` + `efi_switch_mm` is **not**
required — just keep these regions mapped in the kernel tables. (Document the Linux-grade isolation
as a future enhancement.)

**Phase 3 — Map the GOP framebuffer explicitly.**
Map `[fb_info.base_address, +fb_info.size)` (ideally write-combining) in the kernel tables; today it
relies on the inherited firmware mapping.

**Phase 4 — Drop the firmware sub-tables and switch CR3.**
Once the kernel tables cover all RAM + the firmware/runtime/MMIO set + framebuffer + recursive slot
511, switch CR3 to the kernel PML4. The firmware PDPT/PD/PT frames are no longer referenced →
`reserve_firmware_page_tables()` can be **removed**, returning those frames to the PMM (a memory win).
Keep the full-clone path behind a build flag/const as a fallback until HW-validated.

**Phase 5 — Permission hardening (folds in the old "writable everything" concern).**
W^X across the kernel-owned map: kernel code RO+X, data/direct-map NX, framebuffer/MMIO NX. (`EFER.NXE`
is already enabled.)

**Phase 6 — Validation.**
- Unit-test the *pure* table-builder (map-all-RAM frame math + region classification) in the
  `page_table_test.rs` / `pmm_uefi_test.rs` style.
- **Real hardware is mandatory and the only real test** — run the `docs/uefi.md` smoke-test
  checklist on the AMD/UEFI box after Phase 4 and again after Phase 5. Expect the possibility of an
  SMM-class reset and be ready to bisect the kept-region set.

### Risks

- **HW-only validatable.** QEMU tolerates any map; only the real AMD box exercises the SMM path.
- **Platform-specific firmware set.** `EFI_MEMORY_RUNTIME` + reserved/NVS is the *documented* safe
  superset, but if a specific board's SMI touches something outside it, expect a reset → widen the
  kept set or fall back to the full clone.
- **Huge-page support** in the VMM is a real prerequisite for a cheap large-RAM direct map.
