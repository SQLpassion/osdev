# UEFI Bring-up — Outstanding Work (Handoff)

This is a self-contained starting point for continuing the UEFI boot work in a fresh session.
It covers **robustness fixes to what was just built (1)**, **tests still to add (3)**, and
**cleanup (4)**. (The bigger "make UEFI actually usable" items — GOP console, AHCI/NVMe driver,
wiring the post-init path — are tracked separately and are not in this document.)

## Background you need first (read these)

- `docs/uefi.md` §3 — the full UEFI boot + hand-off + kernel-init walkthrough.
- `docs/vmm.md` §4 — why `vmm::init` clones the firmware PML4 (the SMM/SMI reset lesson).
- `docs/pmm.md` §2 (UEFI layout) and §4 — where PMM metadata lives on the UEFI path.

**State as of this handoff:** the kernel boots on real AMD/UEFI hardware through the CR3 switch +
heap + PCI + timer init, then (on a GOP boot) stops in a black/white framebuffer heartbeat. The
fix was: `vmm::init` builds the kernel PML4 as a **superset of the firmware PML4** (clone all 512
entries, add a recursive self-map at slot 511) instead of a minimal hand-built map. Key files:

- `kernel/src/memory/vmm/page_table.rs` — `build_kernel_pml4_from_firmware(src, dst, dst_phys)`
  (the extracted, unit-tested clone logic) + `RECURSIVE_SLOT = 511`.
- `kernel/src/memory/vmm/mod.rs` — `vmm::init` calls the above, then `write_cr3`.
- `kernel/src/memory/pmm/manager.rs` — `PhysicalMemoryManager::new()` chooses the metadata base
  (`BootInfo.pmm_metadata_base` on UEFI, else after `__bss_end`) and reserves used ranges.
- `kernel/src/memory/pmm/types.rs` — `KERNEL_OFFSET = 0x100000`, `STACK_TOP = 0x400000`.

Constraint to remember: physical RAM (incl. the cloned firmware tables, BootInfo, and the
PMM-metadata region) stays reachable after the switch only because the kernel PML4 keeps the
firmware **identity map** (`PML4[0]`). Do not break that.

---

## 1. Robustness fixes to the page-table / PMM rework

### 1a. Split the PMM "used" reservation into two ranges (don't reserve one giant span)

**Where:** `kernel/src/memory/pmm/manager.rs`, end of `PhysicalMemoryManager::new()` (~line 205):

```rust
let reserved_end = align_up(metadata_end.max(STACK_TOP), PAGE_SIZE);
pmm.mark_range_used(KERNEL_OFFSET, reserved_end);
```

**Problem (UEFI only):** `metadata_end = pmm_metadata_base + bitmaps`, and on UEFI the loader puts
`pmm_metadata_base` tens of GiB up (`AllocateAnyPages`). So this single call marks **all RAM from
1 MiB up to the metadata region as used** — safe, but it wastes most of RAM and pushes the first
real allocations (e.g. the kernel PML4 frame) far up. On the BIOS path metadata sits just past the
kernel, so the span is small and this is fine.

**Fix:** reserve **two separate ranges** instead of one span:
1. the low kernel + bootstrap-stack block: `[KERNEL_OFFSET, STACK_TOP)` (i.e. `0x100000..0x400000`),
2. the PMM-metadata region: `[align_down(pmm_metadata_base), align_up(metadata_end))`.
   On the BIOS/fallback path (`metadata_base == kernel_end_phys`), keep the existing single-span
   behavior (or reserve `[KERNEL_OFFSET, metadata_end]`), since the metadata is contiguous with the
   kernel there.

**Acceptance criteria:**
- After `new()`, frames inside both reserved ranges are marked used; frames *between* them (normal
  RAM above `STACK_TOP` and below the metadata region) are **free** and allocatable.
- BIOS path behavior unchanged (existing `pmm_test` still passes).
- This unblocks test 3a.

### 1b. Protect the cloned firmware page-table frames from the PMM (most urgent)

**Why:** `build_kernel_pml4_from_firmware` copies the firmware's top-level entries, so the kernel
PML4 now points at **firmware-owned PDPT/PD/PT frames**. The PMM does not know these frames are in
use and may hand them out via `alloc_frame()`, after which a later write corrupts the live page
tables — a sporadic, hard-to-debug failure that will appear once enough frames are allocated.

**Options (pick one):**
- **(preferred, smaller)** During boot, walk the firmware PML4 (the present entries and their
  sub-tables, to the depth the firmware uses — watch for huge-page leaves via `PageTableEntry::huge()`)
  and `mark_range_used()` every table frame so the PMM never reuses them. Do this right after the
  PMM is initialized and before any significant allocation.
- **(larger, the long-term clean design)** Rebuild kernel-owned page tables as a *proper superset*:
  allocate kernel frames for the levels you need, map only the regions actually required
  (kernel image, identity of RAM the kernel uses, MMIO, **and the firmware/SMM/ACPI regions the
  platform needs** — see `vmm.md` §4.3), and drop the rest. This also enables item 4a.

**Acceptance criteria:** after heavy allocation (e.g. allocate+free thousands of frames, then
exercise the VMM), the page tables remain intact and the kernel stays stable. No frame returned by
`alloc_frame()` ever overlaps a live page-table frame.

---

## 3. Tests still to add

The test harness: integration tests in `kernel/tests/*.rs`, each a `#![no_std]`/`#![no_main]`
kernel with a `KernelMain` entry, `#[test_runner(kaos_kernel::testing::test_runner)]`, and
`#[test_case]` functions; registered as a `[[test]]` in `kernel/Cargo.toml`; run via
`cargo test --test <name>` (boots in QEMU through `tests/test_runner.sh`). See the existing
`tests/boot_info_layout_test.rs` and `tests/page_table_test.rs` (added in the rework) for the
pattern, and `tests/pmm_test.rs` for the PMM (BIOS-path) pattern (`pmm::init` + `pmm::with_pmm`).

### 3a. PMM with a synthetic UEFI memory map  *(depends on 1a + a pub accessor)*

Currently blocked by two things:
- the giant-span reservation (1a) — fix it first, otherwise a synthetic low region is fully
  reserved and nothing is allocatable in the test, and
- `PhysicalMemoryManager::regions()` is **private** (`manager.rs:29`) — add a `pub` read-only
  accessor (e.g. `pub fn regions_snapshot(&self) -> &[PmmRegion]`) so a test can inspect parsed
  regions/bitmaps.

**Then** add `tests/pmm_uefi_test.rs` that, before `pmm::init`, publishes a synthetic `BootInfo`
(via `boot_info::BOOT_INFO_PTR`) pointing at:
- a `static UnifiedMemoryEntry[]` with a mix: a usable region `>= KERNEL_OFFSET`, a usable region
  below `KERNEL_OFFSET` (must be filtered out), and a non-usable region, and
- a page-aligned `static` buffer for `pmm_metadata_base` (large enough for header + regions +
  bitmaps of the synthetic map).

Assert: the usable filter (`is_usable && start >= KERNEL_OFFSET`), `region_count`, each region's
`start`/`frames_total`, that bitmaps were zeroed, the two reserved ranges from 1a, and that an
`alloc_frame()` returns a PFN inside a usable region and outside the reserved ranges. (Do **not**
dereference allocated frames — the addresses are synthetic.)

### 3b. Metadata-base selection logic

A focused test that the PMM picks `BootInfo.pmm_metadata_base` when non-zero, and falls back to
"after `__bss_end`" when there is no BootInfo / the field is 0. May require extracting the
selection into a small pure helper in `manager.rs` to test without side effects.

### 3c. Real-hardware smoke-test checklist (documentation, not a unit test)

The actual real-HW failure mode (the SMM/SMI reset) is **not reproducible in QEMU or any unit
test**. Document a manual checklist (build → `dd` to USB → boot on the UEFI target → expect the
boot markers then the black/white heartbeat) so HW validation is a repeatable, recorded step. Put
it in `docs/uefi.md` (near "Real hardware") or a short `docs/uefi_hw_checklist.md`.

### 3d. GOP framebuffer console tests *(only once that console exists — separate work item)*

Pure geometry/stride/bounds tests for a future framebuffer console (glyph placement, scrolling,
`pixels_per_scanline` vs `width`). Listed here for completeness; depends on the console existing.

---

## 4. Cleanup / lower priority

### 4a. Trim the kernel address space (don't keep the full firmware map forever)

Today the kernel inherits the firmware's *entire* PML4 (full identity of all RAM + every firmware
region, all writable). That is deliberate for bring-up. A cleaner design (ties into 1b option 2)
builds a kernel-owned superset that maps only what's needed plus the required firmware/SMM/MMIO/
ACPI regions, and drops the rest. Not urgent; do it together with 1b's "rebuild" option if chosen.

### 4b. Remove the temporary bring-up visuals

Once a real console / boot path exists on UEFI, remove the temporary diagnostics in
`kernel/src/main.rs`: the early GOP **color gradient**, the end-of-boot **black/white heartbeat**
loop, and the `booted_via_gop` / `fill_screen` helpers that only serve it. Replace with normal
console output. (The per-phase color markers, the `dbg_trap` catch-all, the validator, and the
`vmm_dbg_*` helpers were already removed during the rework — do not re-introduce them.)

---

## Suggested order

1. **1b** (firmware-frame protection) — most urgent; prevents sporadic page-table corruption.
2. **1a** (split reservation) + **3a/3b** (PMM UEFI tests) — robustness + regression coverage.
3. **3c** (HW checklist) — cheap, documents the only way to validate the SMM-class behavior.
4. **4a/4b** — cleanup, ideally folded into 1b's "rebuild" option.
