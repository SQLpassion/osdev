# KAOS Rust VMM - Detailed Implementation Guide

This document describes the **current** implementation in:

- `src/memory/vmm.rs`
- `src/arch/interrupts.rs` (page-fault ISR wiring)
- `src/logging.rs` (targeted logging/capture used by VMM debug output)

It is written as a technical onboarding guide, including concrete address examples and page-walk diagrams.

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
The bootloader (`main64/kaosldr_16/longmode.asm`) initially uses 2 MiB huge pages to enter long mode quickly.

After `vmm::init()` builds new page tables and switches CR3, those bootloader tables are no longer active.

**Therefore: after VMM initialization, the Rust kernel does not use huge pages.**

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

## 4) Initial VMM setup (`vmm::init`)

`vmm::init(debug_output)` allocates and initializes paging structures, then switches CR3.

Allocated table frames:

- 1x PML4
- Identity branch: 1x PDP + 1x PD + 2x PT
- Higher-half branch: 1x PDP + 1x PD + 2x PT

Then:

- `PML4[0]` -> identity branch
- `PML4[256]` -> higher-half branch
- `PML4[511]` -> points to PML4 itself (recursive mapping)

Mapped ranges:

- identity `0..4 MiB`
- higher-half mirror of `0..4 MiB` at `0xFFFF_8000_0000_0000 + offset`

Finally:

- `write_cr3(pml4_phys)` activates the new page tables.

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

`handle_page_fault(virtual_address, error_code)`:

1. Align VA down to page boundary.
2. Optional debug logs (raw/aligned VA, CR3, indexes, error bits).
3. `ensure_tables_for(va)` ensures intermediate levels exist (PML4/PDP/PD entries).
4. If final PT entry absent, allocate one physical frame and map it.

After return, CPU retries the faulting instruction and access succeeds.

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

### 7.2 Step-by-step (what `ensure_tables_for` + handler do)

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
- Set final PT entry to provided physical frame.
- `invlpg(va)` to invalidate stale TLB entry.

### `unmap_virtual_address(va)`

- Align VA.
- Find PT via recursive window.
- Clear PT entry if present.
- `invlpg(va)`.

Note: unmapping currently removes translation only; it does not return the old data frame to PMM.

---

## 10) VMM logging and console debug output

VMM logs now use the centralized logger (`src/logging.rs`) with target `"vmm"`:

- `logging::logln("vmm", format_args!(...))`

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
- VMM smoke command + integration tests

Not yet implemented:

- process-specific address spaces / cloning
- frame reclamation on unmap
- COW/shared-page policies
- SMP-aware TLB shootdown
