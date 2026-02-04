# Virtual Memory Manager (VMM) in KAOS Rust Kernel

This document is a deep technical guide to the current VMM implementation in `src/memory/vmm.rs`.

It is written for developers who are new to paging and to this codebase. After reading it, you should be able to:

- explain how virtual memory translation works in this kernel,
- understand why and how page faults are handled,
- understand recursive page-table mapping and the exact address formulas used,
- reason about `map_virtual_to_physical` / `unmap_virtual_address`,
- debug the common failure modes (wrong mask, wrong CR3, missing mappings).

---

## 1) Mental model: virtual memory vs physical memory

### 1.1 Physical memory
Physical memory is RAM, addressed by physical addresses. The PMM (`src/memory/pmm.rs`) allocates physical page frames of 4 KiB (`PAGE_SIZE = 4096`).

The VMM never asks "where is free virtual memory?"; it asks PMM for physical frames and wires them into page tables.

### 1.2 Virtual memory
The CPU executes using **virtual addresses**. Translation to physical addresses happens in hardware via page tables.

For this kernel:

- page size: 4 KiB
- 4-level paging on x86_64
- 512 entries per table
- levels: `PML4 -> PDP -> PD -> PT -> page`

### 1.3 Why we need this
Virtual memory gives us:

- stable higher-half kernel addresses (`0xFFFF_8000_...`),
- sparse mappings (map only touched pages),
- access-control bits (present/read-write/user),
- fault-driven on-demand allocation.

---

## 2) x86_64 paging structure in this implementation

## 2.1 Bit-level breakdown of a 48-bit canonical virtual address

For 4 KiB pages:

- bits `47..39` -> `PML4 index`
- bits `38..30` -> `PDP index`
- bits `29..21` -> `PD index`
- bits `20..12` -> `PT index`
- bits `11..0`  -> page offset

In code (see `vmm.rs`):

- `pml4_index(va)`
- `pdp_index(va)`
- `pd_index(va)`
- `pt_index(va)`

### 2.2 Visual decomposition

```text
63                    48 47         39 38         30 29         21 20         12 11        0
+-----------------------+-------------+-------------+-------------+-------------+-----------+
| sign extension        | PML4 index  | PDP index   | PD index    | PT index    | offset    |
+-----------------------+-------------+-------------+-------------+-------------+-----------+
```

### 2.3 Concrete decomposition example

For `VA = 0xFFFF_8034_C232_C000`:

- `PML4 = 256`
- `PDP  = 211`
- `PD   = 17`
- `PT   = 300`
- `offset = 0`

This is one of the `vmmtest` addresses and is intentionally far from kernel bootstrap pages.

---

## 3) Page-table entries used by this VMM

`PageTableEntry` is a transparent newtype over `u64`.

Bits used:

- bit 0: Present
- bit 1: Writable
- bit 2: User
- bits 12..: frame base (`ENTRY_FRAME_MASK = 0x0000_FFFF_FFFF_F000`)

The code uses explicit setters/getters instead of C-style bitfields.

Important behavior:

- the implementation maps kernel pages as supervisor (`user = false`),
- writable is set for all current kernel mappings,
- `clear()` zeroes the entire entry.

---

## 4) Boot-time VMM initialization (`vmm::init`)

`vmm::init(debug_output)` builds a minimal but usable page-tree and then switches CR3.

### 4.1 Frames allocated

Current implementation allocates nine frames:

- `pml4`
- identity branch: `pdp_identity`, `pd_identity`, `pt_identity_0`, `pt_identity_1`
- higher-half branch: `pdp_higher`, `pd_higher`, `pt_higher_0`, `pt_higher_1`

All are explicitly zeroed with `write_bytes`.

### 4.2 Top-level wiring

- `PML4[0]   -> identity branch`
- `PML4[256] -> higher-half branch`
- `PML4[511] -> PML4 itself` (**recursive mapping**)

### 4.3 Initial mapped regions

Both identity and higher-half branch map `0..4 MiB`:

- first PT maps frames `0..511` (0..2 MiB)
- second PT maps frames `512..1023` (2..4 MiB)

This is a practical stability choice (stack / early runtime after CR3 switch).

### 4.4 CR3 switch ordering

The kernel startup order in `src/main.rs` is intentional:

1. `pmm::init()`
2. `interrupts::init()` (IDT incl. PF handler installed)
3. `vmm::init(false)` (CR3 switch)
4. later: register IRQ handlers, enable interrupts

If page-fault handling were not installed before risky memory accesses, an early PF could become a triple fault reset.

---

## 5) Recursive mapping: what it is and why we need it

## 5.1 Problem without recursion

Page tables are physical-memory structures. Without recursion, modifying them needs either:

- temporary ad-hoc mappings, or
- assumptions about identity-mapped physical memory.

Both are brittle.

### 5.2 Recursive trick

Set `PML4[511]` to the PML4's own physical frame.

That creates a virtual region where the page-table hierarchy becomes self-addressable. In other words: page tables are mapped into virtual space, so the kernel can treat them as normal pointers.

### 5.3 Windows used in this implementation

Constants:

- `PML4_TABLE_ADDR = 0xFFFF_FFFF_FFFF_F000`
- `PDP_TABLE_BASE  = 0xFFFF_FFFF_FFE0_0000`
- `PD_TABLE_BASE   = 0xFFFF_FFFF_C000_0000`
- `PT_TABLE_BASE   = 0xFFFF_FF80_0000_0000`

Address helpers:

- `pdp_table_addr(va) = PDP_TABLE_BASE + ((va >> 27) & 0x0000_001F_F000)`
- `pd_table_addr(va)  = PD_TABLE_BASE  + ((va >> 18) & 0x0000_3FFF_F000)`
- `pt_table_addr(va)  = PT_TABLE_BASE  + ((va >>  9) & 0x0000_007F_FFFF_F000)`

The masks are critical. A wrong mask means wrong table address, which causes repeated page faults or full system reset.

### 5.4 Example of computed recursive addresses

For `VA = 0xFFFF_8034_C232_C000`, the helper windows resolve to:

- PDP table VA: `0xFFFF_FFFF_FFF0_0000`
- PD table VA : `0xFFFF_FFFF_E00D_3000`
- PT table VA : `0xFFFF_FFC0_1A61_1000`

These are **virtual addresses to table pages**, not to payload data pages.

---

## 6) Page faults in this kernel

## 6.1 Fault entry path

In `src/arch/interrupts.rs`:

- IDT vector 14 points to `isr14_stub`.
- stub saves registers,
- reads `CR2` into first arg register (`rdi`),
- reads fault error code from interrupt stack (`[rsp + 120]`) into `rsi`,
- calls `page_fault_handler_rust(fault_va, error_code)`.

Then Rust dispatches to `vmm::handle_page_fault(...)`.

### 6.2 Error-code bits (x86 page-fault error code)

Current debug output decodes:

- `p`      (bit 0): protection violation vs non-present
- `w`      (bit 1): write access
- `u`      (bit 2): user-mode access
- `rsv`    (bit 3): reserved bit set in paging structures
- `ifetch` (bit 4): instruction fetch fault

### 6.3 Handler behavior (`handle_page_fault`)

1. align fault address to page boundary,
2. log raw/aligned address, CR3, error bits (if debug enabled),
3. call `ensure_tables_for(va)` to allocate missing intermediate tables,
4. allocate final data frame if PT entry not present,
5. set PT entry present+writable+supervisor.

The faulting instruction is then retried by CPU and should succeed.

---

## 7) How `ensure_tables_for` works

`ensure_tables_for(va)` walks top-down through recursive windows:

- get PML4 via `PML4_TABLE_ADDR`,
- if needed, allocate missing PML4 entry target frame,
- access corresponding PDP table via `pdp_table_addr(va)`,
- if needed, allocate missing PDP entry target frame,
- access corresponding PD table via `pd_table_addr(va)`,
- if needed, allocate missing PD entry target frame,
- zero each newly created table page.

`invlpg(...)` is used when moving into freshly created next-level views to avoid stale translation artifacts.

---

## 8) Mapping a VA to an explicit PA

Function: `map_virtual_to_physical(virtual_address, physical_address)`

Steps:

1. align VA and PA to 4 KiB,
2. ensure all intermediate levels exist,
3. write PT entry with target PFN,
4. `invlpg(va)`.

This is explicit mapping (caller chooses PA), unlike demand paging in PF handler (VMM allocates PA).

---

## 9) Unmapping a VA

Function: `unmap_virtual_address(virtual_address)`

Steps:

1. align VA,
2. compute PT window address,
3. clear PT entry if present,
4. `invlpg(va)`.

Important: current implementation does **not** free the physical frame backing that page. It only removes translation.

---

## 10) `vmmtest` behavior (command + semantics)

Shell command in `src/main.rs`:

- `vmmtest`
- `vmmtest --debug`
- (`testvmm` alias kept for compatibility)

`vmm::test_vmm()` writes to three far-distributed higher-half addresses:

- `0xFFFF_8009_4F62_D000` (`PML4=256, PDP=37,  PD=123, PT=45`)
- `0xFFFF_8034_C232_C000` (`PML4=256, PDP=211, PD=17,  PT=300`)
- `0xFFFF_807F_7200_7000` (`PML4=256, PDP=509, PD=400, PT=7`)

It then:

1. reads values back,
2. verifies expected bytes,
3. unmaps those three pages,
4. returns success/failure.

Because pages are unmapped at the end, next test run triggers page faults again (repeatable test cycle).

---

## 11) Debug output architecture

VMM logging has two paths:

1. serial (`vmm_logln!` always writes serial)
2. optional console capture buffer (enabled by `set_console_debug_output(true)`)

Console dump is printed via `print_console_debug_output(screen)`.

Color policy:

- green: lines beginning with
  - `VMM: page fault raw=`
  - `VMM: indices pml4=`
- white: all other VMM lines

`vmmtest --debug` enables capture and prints the buffered block to VGA console.

---

## 12) Integration tests

`tests/vmm_test.rs` adds dedicated integration coverage:

- `test_vmm_smoke_once`
- `test_vmm_smoke_twice`

Test kernel initializes:

- serial
- PMM
- interrupts (IDT/PF handler)
- VMM

Then runs `vmm::test_vmm()` and asserts success.

The second test validates repeatability of map/fault/unmap cycle.

---

## 13) Safety boundaries and invariants

Unsafe areas are localized around hardware and raw memory operations:

- `read_cr3` / `write_cr3` / `invlpg` inline asm
- raw pointer table access (`table_at`)
- raw memory zeroing (`zero_phys_page`)

Critical invariants:

- PMM returns valid 4 KiB-aligned frames,
- recursive entry `PML4[511]` always points to active PML4 frame,
- recursive masks remain exact,
- page-fault handler is installed before vulnerable accesses,
- CR3 only switched to valid, fully initialized page trees.

Violation symptoms:

- repeated PFs at strange recursive addresses,
- faults with non-sensical indices,
- immediate reset/triple-fault.

---

## 14) Scope and current limitations

Implemented today:

- kernel 4-level paging,
- higher-half + identity bootstrap mappings,
- recursive mapping,
- demand page allocation on PF,
- explicit map/unmap helpers,
- command-level and integration tests.

Not yet implemented:

- cloning/switching per-process address spaces,
- user-space memory management policy,
- physical-frame reclamation on unmap,
- copy-on-write/shared-page semantics,
- advanced TLB shootdown strategy for SMP (system is currently single-core oriented).

---

## 15) Practical debugging checklist

When VMM behavior looks wrong, check in this order:

1. Is `EXCEPTION_PAGE_FAULT` (14) installed in IDT before CR3 switch?
2. Is `PML4[511]` recursive entry valid?
3. Are recursive masks exactly those shown above?
4. Do debug logs show plausible indices for your fault VA?
5. Is the fault error code `p/w/u/rsv/ifetch` consistent with expectation?
6. After map/unmap changes, did `invlpg` run on the affected VA?

This checklist catches most practical regressions in this code path.
