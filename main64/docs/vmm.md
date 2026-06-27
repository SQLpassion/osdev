# KAOS Rust VMM - Detailed Implementation Guide

This document describes the **current** implementation in:

- `src/memory/vmm.rs`
- `src/arch/interrupts.rs` (page-fault ISR wiring)
- `src/logging.rs` (targeted logging/capture used by VMM debug output)

It is written as a technical onboarding guide, including concrete address examples and page-walk diagrams.

---

## Table of Contents
- [1) Virtual memory vs physical memory (in this kernel)](#1-virtual-memory-vs-physical-memory-in-this-kernel)
- [2) x86_64 page hierarchy used here](#2-x86_64-page-hierarchy-used-here)
- [3) Page-table entry representation](#3-page-table-entry-representation)
- [4) Initial VMM setup (`vmm::init`) — cloning the firmware PML4](#4-initial-vmm-setup-vmminit--cloning-the-firmware-pml4)
  - [4.1 What it does now](#41-what-it-does-now)
  - [4.2 What "clone the PML4" actually copies](#42-what-clone-the-pml4-actually-copies)
  - [4.3 Why a minimal map reset real hardware (and the clone does not)](#43-why-a-minimal-map-reset-real-hardware-and-the-clone-does-not)
  - [4.4 Known caveat (open follow-up)](#44-known-caveat-open-follow-up)
- [5) Page faults: how the kernel handles them](#5-page-faults-how-the-kernel-handles-them)
  - [5.1 Interrupt wiring](#51-interrupt-wiring)
  - [5.2 Demand paging behavior](#52-demand-paging-behavior)
- [6) Recursive page mapping: concept and mechanics](#6-recursive-page-mapping-concept-and-mechanics)
  - [6.1 Why it exists](#61-why-it-exists)
  - [6.2 Recursive windows used in current code](#62-recursive-windows-used-in-current-code)
- [7) Concrete example: mapping a virtual address using recursive mapping](#7-concrete-example-mapping-a-virtual-address-using-recursive-mapping)
- [8) ASCII diagrams: page walk and recursive modification](#8-ascii-diagrams-page-walk-and-recursive-modification)
  - [8.1 Hardware page walk for one VA](#81-hardware-page-walk-for-one-va)
  - [8.2 How recursive mapping gives virtual access to those tables](#82-how-recursive-mapping-gives-virtual-access-to-those-tables)
- [9) Mapping and unmapping APIs](#9-mapping-and-unmapping-apis)
- [10) VMM logging and console debug output](#10-vmm-logging-and-console-debug-output)
- [11) `vmmtest` behavior (current)](#11-vmmtest-behavior-current)
- [12) Safety boundaries and critical invariants](#12-safety-boundaries-and-critical-invariants)
- [13) Scope and limitations (as of current code)](#13-scope-and-limitations-as-of-current-code)

---

## 1) Virtual memory vs physical memory (in this kernel)

### Physical memory
Physical memory is RAM addressed by physical addresses. Frames are 4 KiB (`SMALL_PAGE_SIZE = 4096`).

The PMM (`src/memory/pmm.rs`) is responsible for allocating/freeing physical frames.

### Virtual memory
CPU memory accesses are made with virtual addresses. x86_64 hardware translates virtual -> physical using page tables referenced by CR3.

In this kernel:

- Paging mode: x86_64, 4-level paging
- Page size in the Rust VMM: **4 KiB**
- Table fanout: 512 entries per level

### Important note about huge pages

This differs between the two boot paths:

- **Legacy BIOS path** (`main64/kaosldr_16/longmode.asm`): the bootloader uses 2 MiB huge pages
  to enter long mode quickly. `vmm::init()` then builds fresh 4 KiB tables and switches CR3, so
  the bootloader's huge-page tables are no longer active.

- **UEFI path:** `vmm::init()` does **not** build a map from scratch — it **clones the firmware's
  top-level page table** (see §4). The firmware's identity / higher-half mappings often use
  **1 GiB or 2 MiB huge pages**, so after the CR3 switch the *active* tables for those regions
  may still be huge pages. The kernel does not mind: the hardware page walker handles any page
  size, and the recursive window the VMM uses for its *own* `map`/`unmap` operations always
  creates 4 KiB page tables.

**Bottom line:** the kernel's own mappings (heap, user space, on-demand pages) are always 4 KiB;
the inherited firmware identity/higher-half mappings on the UEFI path may be huge pages.

---

## 2) x86_64 page hierarchy used here

For canonical 48-bit virtual addresses:

- bits 47..39 -> PML4 index
- bits 38..30 -> PDP index
- bits 29..21 -> PD index
- bits 20..12 -> PT index
- bits 11..0 -> offset inside page

In `vmm.rs` this is:

- `pml4_index(va)`
- `pdp_index(va)`
- `pd_index(va)`
- `pt_index(va)`

### ASCII view of address decomposition

```text
63                    48 47         39 38         30 29         21 20         12 11         0
+-----------------------+-------------+-------------+-------------+-------------+------------+
| sign extension        | PML4 index  | PDP index   | PD index    | PT index    | page off   |
+-----------------------+-------------+-------------+-------------+-------------+------------+
```

---

## 3) Page-table entry representation

`PageTableEntry(u64)` wraps one 64-bit entry and exposes bit-level helpers.

Used flags:

- Present (`bit 0`)
- Writable (`bit 1`)
- User (`bit 2`)
- Frame base (`bits 12..`, via `ENTRY_FRAME_MASK`)

`set_mapping(pfn, present, writable, user)` writes frame + flags in one call.

Current kernel mappings in VMM are created as supervisor (`user = false`).

---

## 4) Initial VMM setup (`vmm::init`) — cloning the firmware PML4

> **History / important:** earlier versions of `vmm::init` hand-built a *minimal* map from
> scratch (allocate a fresh PML4 + an identity branch covering `0..4 MiB` + a higher-half branch
> + a recursive entry, then switch CR3). That design **worked in QEMU but instantly reset real
> AMD UEFI hardware** the moment CR3 was loaded. The current implementation instead **clones the
> firmware's top-level page table**. The reasoning below is the hard-won result of a long
> bisection; read it before "simplifying" this code.

### 4.1 What it does now

When `KernelMain` calls `vmm::init`, the **firmware's page tables are still active** in CR3 (the
UEFI loader handed off without changing CR3; it only mirrored `PML4[0]→PML4[256]` to create the
higher half — see [`uefi.md`](uefi.md) §3.6). Because the firmware identity-maps RAM
(physical == virtual), a physical address can be dereferenced directly as a pointer.

`vmm::init` therefore does, in full:

```rust
// 1. Allocate + zero one fresh frame for the kernel's own PML4.
let pml4 = alloc_frame_phys_or_panic(...);
zero_phys_page(pml4);

// 2. Find the firmware PML4 (CR3 holds its physical address; mask off flag bits).
let fw_pml4 = read_cr3() & 0x000F_FFFF_FFFF_F000;

// 3. Copy ALL 512 top-level entries from the firmware PML4 into ours.
//    (phys == virt here, so table_at(x) just treats x as a *mut PageTable.)
for i in 0..512 {
    *entry_ptr(table_at(pml4), i) = *entry_ptr(table_at(fw_pml4), i);
}

// 4. Overwrite slot 511 with our recursive self-map (PML4[511] -> the PML4 itself).
(*entry_ptr(table_at(pml4), 511)).set_mapping(phys_to_pfn(pml4), true, true, false);

// 5. Publish state and activate the new root.
write_cr3(pml4);
```

That is the **entire** function — about a dozen lines. There is no hand-built identity or
higher-half branch anymore.

### 4.2 What "clone the PML4" actually copies

A PML4 is a single 4 KiB page of **512 entries × 8 bytes**. Each entry is a `u64` holding the
**physical address of the next-level table (a PDPT)** plus flags. Copying the 512 entries copies
only the **top-level pointers** — *not* the whole multi-level tree. Our new PML4 therefore points
at the **same** PDPT/PD/PT sub-tables the firmware built; we share its entire lower hierarchy by
reference. This works because page-table entries store **absolute physical addresses**, which are
independent of which PML4 is currently in CR3.

After step 5, the kernel's address space contains, simultaneously:

| Slot      | Mapping                                                            | Origin                         |
|-----------|-------------------------------------------------------------------|--------------------------------|
| `PML4[0]` | **identity** (virt == phys) of low memory / all RAM               | cloned from firmware           |
| `PML4[256]` | **higher-half** mirror (`0xFFFF8000…` → same physical as `0x0…`) | firmware slot the loader set   |
| `PML4[511]` | **recursive** self-map (the VMM's window onto its own tables)    | written by us                  |
| others    | SMM / ACPI / MMIO / firmware runtime regions                      | cloned from firmware           |

So the **identity mapping stays active** after the switch. The kernel *code* runs in the higher
half (`PML4[256]`), but any physical address is still reachable directly via `PML4[0]`. The
end-of-boot framebuffer heartbeat and reads of the loader-provided `BootInfo` (which lives high in
RAM) both rely on this.

### 4.3 Why a minimal map reset real hardware (and the clone does not)

Bisection on the real machine established, with certainty:

- Switching to a **byte-identical clone of the firmware PML4** (even allocated in a high physical
  frame) → **works**.
- Clone **+ our recursive `PML4[511]`** → **works**.
- Our **hand-built minimal map** (only `0..4 MiB` identity + higher-half + recursive, everything
  else absent) → **instant hard reset at the `write_cr3`**, with **no CPU exception at all** (a
  catch-all installed on every IDT vector 0–31 caught nothing; it was not `#PF`/`#GP`/`#MC`/NMI),
  and **only on real hardware** (QEMU always tolerated it).

A reset that bypasses the IDT entirely, only on real hardware, points at **System Management Mode
(SMM)**: real firmware leaves SMM active and takes asynchronous **SMIs** (power/thermal/USB-legacy
emulation). The platform's SMM path depends on the firmware's memory mappings; a minimal kernel
map discards them, so the next SMI faults inside SMM and the platform hard-resets. QEMU has no
such SMM activity. Cloning the firmware PML4 keeps every mapping the platform might need, so SMIs
continue to resolve.

This also fixes two latent bugs for free: the loader-provided **`BootInfo`** and the high
**PMM-metadata region** (see [`pmm.md`](pmm.md)) both live in RAM that `PML4[0]` identity-maps, so
they remain reachable after the switch — a hand-built 4 MiB map would have lost them.

### 4.4 Known caveat (open follow-up)

The cloned sub-tables are **firmware-owned frames** that the PMM does **not** know are in use, so
it could later hand them out and corrupt the page tables. For the current bring-up (boot to a
stable idle/heartbeat) this has not bitten, but a future "clean" address space should either
reserve those frames or rebuild kernel-owned tables as a *proper superset* that still covers the
critical firmware/SMM/MMIO regions. Do not assume the firmware sub-tables are private to the
kernel.

---

## 5) Page faults: how the kernel handles them

## 5.1 Interrupt wiring

In `src/arch/interrupts.rs`:

- IDT vector 14 (`EXCEPTION_PAGE_FAULT`) uses `isr14_stub`.
- Stub reads:
  - `CR2` (faulting VA) -> first argument
  - page-fault error code from stack -> second argument
- Calls Rust handler: `page_fault_handler_rust(faulting_address, error_code)`
- Rust dispatches to `vmm::handle_page_fault(...)`.

## 5.2 Demand paging behavior

The VMM now has two page-fault entry points:

- `try_handle_page_fault(virtual_address, error_code) -> Result<(), PageFaultError>`
- `handle_page_fault(virtual_address, error_code)` (production wrapper used by interrupts)

`try_handle_page_fault(virtual_address, error_code)`:

1. Align VA down to page boundary.
2. Optional debug logs (raw/aligned VA, CR3, indexes, error bits).
3. If `error_code` has `P=1` (protection fault), return `Err(PageFaultError::ProtectionFault { .. })` and do **not** allocate.
4. `populate_page_table_path(va)` ensures intermediate levels exist (PML4/PDP/PD entries).
5. If final PT entry is absent, allocate one physical frame, map it, invalidate TLB entry, and zero the new page.

`handle_page_fault(...)` calls `try_handle_page_fault(...)` and panics on `ProtectionFault`, preserving the kernel's fatal behavior for real access violations.

For non-present faults, after return, CPU retries the faulting instruction and access succeeds.

---

## 6) Recursive page mapping: concept and mechanics

Recursive mapping is the key technique enabling easy page-table edits.

## 6.1 Why it exists

Without recursion, page tables are just physical memory. To edit them, kernel would need temporary mappings of each table frame.

With recursion:

- `PML4[511] = PML4 frame`

This creates virtual windows where paging structures are directly addressable as virtual memory.

## 6.2 Recursive windows used in current code

Constants in `vmm.rs`:

- `PML4_TABLE_ADDR = 0xFFFF_FFFF_FFFF_F000`
- `PDP_TABLE_BASE = 0xFFFF_FFFF_FFE0_0000`
- `PD_TABLE_BASE  = 0xFFFF_FFFF_C000_0000`
- `PT_TABLE_BASE  = 0xFFFF_FF80_0000_0000`

Address helper formulas:

- `pdp_table_addr(va) = PDP_TABLE_BASE + ((va >> 27) & 0x0000_001F_F000)`
- `pd_table_addr(va)  = PD_TABLE_BASE  + ((va >> 18) & 0x0000_3FFF_F000)`
- `pt_table_addr(va)  = PT_TABLE_BASE  + ((va >>  9) & 0x0000_007F_FFFF_F000)`

These produce virtual addresses for the relevant table pages for `va`.

---

## 7) Concrete example: mapping a virtual address using recursive mapping

We use one test address from `test_vmm()`:

- `VA = 0xFFFF_8034_C232_C000`

Indexes:

- `pml4 = 256`
- `pdp = 211`
- `pd = 17`
- `pt = 300`
- `offset = 0`

Assume this VA is currently unmapped and triggers a non-present page fault.

### 7.1 Table windows for this VA

Using current formulas, code computes:

- `pdp_table_addr(VA) = 0xFFFF_FFFF_FFF0_0000`
- `pd_table_addr(VA)  = 0xFFFF_FFFF_E00D_3000`
- `pt_table_addr(VA)  = 0xFFFF_FFC0_1A61_1000`

Now the handler can treat these as `*mut PageTable` and write entries.

### 7.2 Step-by-step (what `populate_page_table_path` + handler do)

1. Read PML4 at `PML4_TABLE_ADDR`.
2. Check `PML4[256]`:
   - if not present, allocate frame F1 and set entry to F1.
3. Read PDP table via `pdp_table_addr(VA)`.
4. Check `PDP[211]`:
   - if not present, allocate frame F2 and set entry to F2.
5. Read PD table via `pd_table_addr(VA)`.
6. Check `PD[17]`:
   - if not present, allocate frame F3 and set entry to F3.
7. Read PT table via `pt_table_addr(VA)`.
8. Check `PT[300]`:
   - if not present, allocate data frame F4 and set entry to F4.

Final translation for this VA:

```text
VA 0xFFFF_8034_C232_C000 -> PFN(F4) * 4096 + 0x0
```

---

## 8) ASCII diagrams: page walk and recursive modification

## 8.1 Hardware page walk for one VA

```text
CR3 --> PML4 table
          |
          | index = pml4_index(VA) = 256
          v
        PML4[256] --points to--> PDP table
                                   |
                                   | index = pdp_index(VA) = 211
                                   v
                                 PDP[211] --points to--> PD table
                                                           |
                                                           | index = pd_index(VA) = 17
                                                           v
                                                         PD[17] --points to--> PT table
                                                                               |
                                                                               | index = pt_index(VA) = 300
                                                                               v
                                                                             PT[300] -> physical frame base
                                                                                           + VA offset
```

## 8.2 How recursive mapping gives virtual access to those tables

```text
PML4[511] = PML4 frame (self-reference)

This creates special VA windows:
- fixed VA for PML4 itself
- computed VA for PDP/PD/PT pages corresponding to any target VA

So software can do:
  pt = (pt_table_addr(target_va) as *mut PageTable)
  pt.entries[pt_index(target_va)] = new mapping

instead of creating temporary mappings for table frames.
```

---

## 9) Mapping and unmapping APIs

### `map_virtual_to_physical(va, pa)`

- Align VA and PA to 4 KiB.
- Ensure intermediate levels exist.
- Reject mapping if VA is already mapped to a different PFN.
- Set final PT entry only when currently unmapped.
- `invlpg(va)` to invalidate stale TLB entry.

Checked variant:

- `try_map_virtual_to_physical(va, pa) -> Result<(), MapError>`
- returns `Err(MapError::AlreadyMapped { .. })` on overwrite attempts.

### `unmap_virtual_address(va)`

- Align VA.
- Walk recursive tables with presence checks on PML4/PDP/PD.
- If table path is missing, return without side effects.
- If PT entry is present: capture mapped PFN, clear entry, invalidate TLB entry.
- Return mapped PFN to PMM (`release_pfn`) when the PFN belongs to a managed PMM region.
- `invlpg(va)`.

---

## 10) VMM logging and console debug output

VMM logs use centralized logging (`src/logging.rs`) with target `"vmm"` and are routed through `vmm_logln(...)` in `vmm.rs`.

The VMM now has two independent output channels:

- **Serial output (host/COM1):**
  - controlled by `vmm::init(debug_output)`
  - `vmm::init(true)` enables VMM serial logs
  - `vmm::init(false)` suppresses VMM serial logs

- **In-OS console output (screen dump):**
  - controlled by REPL command flag `vmmtest --debug`
  - implemented via capture + `print_console_debug_output`

`vmmtest --debug` flow:

1. enables log capture,
2. runs `vmm::test_vmm()`,
3. dumps captured target `"vmm"` logs to screen,
4. disables capture.

Coloring rule in VMM console dump:

- green: lines beginning with
  - `VMM: page fault raw=`
  - `VMM: indices pml4=`
- white: all other VMM lines.

---

## 11) `vmmtest` behavior (current)

`test_vmm()` does:

1. write `A/B/C` to three far distributed higher-half addresses,
2. read them back and verify,
3. unmap all three pages,
4. report pass/fail.

Because it unmaps at the end, rerunning `vmmtest` generates page faults again.

---

## 12) Safety boundaries and critical invariants

Unsafe operations are isolated in:

- CR3/TLB asm (`read_cr3`, `write_cr3`, `invlpg`)
- pointer casts to table pages (`table_at`)
- raw memory zeroing (`zero_phys_page`)

Must-hold invariants:

- recursive entry `PML4[511]` is correct,
- recursive masks/formulas are exact,
- page-fault handler is installed before risky post-CR3 accesses,
- PMM returns valid 4 KiB frames.

Typical break symptoms:

- repeated faults with odd recursive indices,
- faults in seemingly unrelated addresses,
- triple-fault/reset.

---

## 13) Scope and limitations (as of current code)

Implemented:

- kernel 4-level paging with 4 KiB pages
- demand paging on non-present faults
- explicit map/unmap API
- recursive page-table mapping
- map overwrite protection via `try_map_virtual_to_physical(...)->Result`
- frame reclamation on unmap (`release_pfn`)
- VMM smoke command + integration tests

Not yet implemented:

- process-specific address spaces / cloning
- COW/shared-page policies
- SMP-aware TLB shootdown
