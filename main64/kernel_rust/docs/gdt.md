# KAOS Rust Kernel: GDT/TSS Deep Technical Documentation

This document explains the Global Descriptor Table (GDT) and Task State Segment (TSS)
in x86_64 long mode, why they are required for user mode, how privilege transitions work,
and how the current KAOS Rust kernel implementation is wired.

Target audience: kernel developers who need exact architectural behavior and implementation
constraints.

---

## 1. Why GDT/TSS Still Matter in x86_64

In 64-bit mode, segmentation is largely disabled for linear address translation, but the CPU
still enforces privilege and control-flow contracts through segment selectors and descriptor
metadata.

Even in long mode, the following remain critical:

- `CS` controls CPL (Current Privilege Level) and execution mode semantics
- `SS`/`DS`/`ES`/`FS`/`GS` selectors must reference valid descriptors
- IDT gate transitions between ring 3 and ring 0 require a valid kernel stack source
- that kernel stack source is `TSS.RSP0`

Without valid GDT/TSS state, ring transitions fail with faults (`#GP`, `#TS`, `#SS`, then
possibly `#DF` and triple fault).

---

## 2. Mental Model

Use this model:

- GDT: descriptor metadata table (segment identities + privilege attributes + TSS descriptor)
- TSS: per-CPU transition metadata (primarily `RSP0` in modern kernels)
- Scheduler: picks next task and updates `TSS.RSP0` to that task's kernel-stack top

One CPU typically has one active TSS.
Each task has its own kernel stack.
`TSS.RSP0` is updated dynamically during task switch.

---

## 3. GDT Entry Layouts

### 3.1 Normal code/data descriptor (8 bytes)

```
63                              56 55    52 51    48 47    40 39          16 15           0
+--------------------------------+--------+--------+--------+--------------+--------------+
| base[31:24]                    | flags  | lim[19:16]      | access byte  | base[23:0]   |
+--------------------------------+--------+--------+--------+--------------+--------------+
|                                  limit[15:0]                                              |
+-------------------------------------------------------------------------------------------+
```

For long mode flat segments in this kernel:

- base = 0
- limit = 0
- `access` carries Present/DPL/Code/Data bits
- upper nibble in the granularity byte carries `L` (64-bit code), etc.

### 3.2 TSS descriptor (16 bytes, consumes 2 GDT slots)

x86_64 TSS descriptor is 128 bits:

```
First 8 bytes  (low):  limit[15:0], base[23:0], type/present, limit[19:16], base[31:24]
Second 8 bytes (high): base[63:32], reserved
```

So one TSS descriptor occupies two consecutive entries in the GDT.

---

## 4. TSS Layout and Purpose in Long Mode

In modern x86_64 kernels, TSS is not used for hardware task switching. It is used for:

- `RSP0`: stack pointer loaded when CPU transitions from ring 3 to ring 0
- `IST1..IST7`: optional emergency stacks for selected IDT entries (e.g. double fault)
- `IOPB`: I/O permission bitmap base

Current KAOS Rust TSS includes the canonical long-mode fields and sets:

- `rsp0` during init
- `io_map_base` to `size_of::<TaskStateSegment>()` (no bitmap in use)

---

## 5. Privilege Transition Flow (Ring 3 -> Ring 0)

When a user task (ring 3) triggers an interrupt/syscall gate to ring 0:

1. CPU checks target descriptor privileges and gate validity
2. CPU loads new stack from `TSS.RSP0`
3. CPU pushes old context to new ring-0 stack:
   - old `SS`
   - old `RSP`
   - `RFLAGS`
   - old `CS`
   - old `RIP`
4. CPU vectors to kernel handler in ring 0

ASCII flow:

```
User task (ring 3) running on user stack
    |
    | interrupt/syscall
    v
CPU reads TR -> active TSS -> RSP0
    |
    v
Switch stack to RSP0 (kernel stack of current task)
    |
    v
Push user return frame (SS,RSP,RFLAGS,CS,RIP)
    |
    v
Enter kernel handler at ring 0
```

If `RSP0` is wrong/unmapped, this path faults immediately.

---

## 6. One TSS per CPU vs One Kernel Stack per Task

No contradiction exists.

- TSS is per CPU (active transition metadata register source)
- kernel stack is per task (scheduler-owned task context memory)
- scheduler updates `TSS.RSP0` to "next task's kernel stack top" before user execution

Conceptual switch path:

```
current task A -> scheduler selects task B
    |
    +--> write TSS.RSP0 = task_b.kernel_stack_top
    +--> restore task B context
    +--> return to task B
```

Then any ring3->ring0 event for B lands on B's kernel stack.

---

## 7. Current KAOS Rust Implementation

### 7.1 Module and state

Implemented in `src/arch/gdt.rs`.

Key state:

- static GDT array (`[u64; 7]`)
- static TSS struct
- atomic initialized flag

Entry plan:

- index 0: null
- index 1: kernel code
- index 2: kernel data
- index 3: user code
- index 4: user data
- index 5: TSS low
- index 6: TSS high

Selector constants:

- kernel code: `0x08`
- kernel data: `0x10`
- user code: `0x1B`
- user data: `0x23`
- tss: `0x28`

### 7.2 Init flow

`gdt::init()` currently:

1. obtains mutable refs to singleton GDT/TSS storage
2. clears GDT and TSS
3. sets `tss.rsp0` to current kernel `rsp`
4. sets `tss.io_map_base`
5. builds descriptors (kernel/user code+data, TSS low/high)
6. builds descriptor table pointer
7. calls assembly helper to load GDT + reload data segments
8. marks initialized flag

### 7.3 Assembly loader behavior

Current assembly helper performs:

- `lgdt [ptr]`
- reload `ds/es/fs/gs/ss`

Important: currently deferred for stability/stepwise rollout:

- no far control transfer to reload `CS`
- no `ltr` yet

This is intentional while bringing up ring-3 support incrementally.

---

## 8. Boot-Time Ordering in KernelMain

Current startup sequence (simplified):

```
serial::init
gdt::init
pmm::init
interrupts::init
vmm::init
heap::init
scheduler setup
interrupts::enable
idle loop
```

GDT init is early so later ring-level assumptions are consistent.

---

## 9. Failure Modes and Triple Fault Mechanism

Typical chain when GDT/TSS setup is invalid:

1. first fault (`#GP` or `#TS`) due to bad descriptor/state
2. fault handler entry itself fails -> `#DF` (double fault)
3. double-fault handler entry fails -> CPU reset (triple fault)

Common root causes:

- wrong descriptor bit placement (incorrect shifts in access/flags bytes)
- invalid TSS descriptor type/present bits
- bad descriptor table limit/base
- loading invalid selectors into segment registers
- invalid/unmapped `RSP0` for ring transition

---

## 10. Debugging Checklist (Practical)

When enabling more of GDT/TSS path (`CS` reload, `ltr`, ring-3 entry), validate in this order:

1. `lgdt` only, no `ltr`, no CS reload
2. data segment reload
3. verify descriptor snapshot bits in tests
4. enable `ltr` with known-good TSS descriptor
5. introduce controlled ring-3 iret frame
6. trigger controlled ring3->ring0 event and verify stack switch

Add serial markers before/after each step to isolate crash boundary.

---

## 11. Current Status in This Repository

What is available now:

- full in-memory GDT/TSS construction logic
- descriptor snapshot/testing hooks
- early boot GDT load + data segment reload
- scheduler uses centralized selector constants

What is intentionally not active yet:

- CS reload path after LGDT
- `ltr` activation
- actual ring-3 task execution
- syscall transition path

This staged approach minimizes regression risk while preparing the architecture for user mode.

---

## 12. ASCII End-to-End Overview

```
+----------------------+        +----------------------+        +----------------------+
| Bootloader enters    |        | Kernel gdt::init     |        | Scheduler / Ring3    |
| long mode with temp  | -----> | builds GDT+TSS,      | -----> | will later update    |
| GDT                  |        | loads GDTR, reloads  |        | TSS.RSP0 per task    |
|                      |        | data segments         |        | before user run      |
+----------------------+        +----------------------+        +----------------------+
                                            |
                                            v
                                  (future staged activation)
                                  - load TR with TSS selector (ltr)
                                  - ring3 task frames
                                  - syscall/interrupt return path
```

