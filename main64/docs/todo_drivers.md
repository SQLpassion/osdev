# Dynamic Driver Infrastructure for KAOS (Option B: User-Space Drivers)

> Status: **Design / planned** — this document describes the architecture and a
> step-by-step implementation plan for loadable device drivers.
>
> Date: 2026-06-17

## 1. Goal

KAOS should be able to **load drivers from the file system at runtime**, without
requiring them to be compiled into the kernel. A driver is just a normal Ring-3
program (a flat binary on the FAT32 disk) that talks to "its" hardware through a
set of **privileged, capability-gated syscalls**.

The end result:

```
shell> load RTL8139.DRV
[driver-manager] spawned as TID 7, capabilities: IO_PORTS | MMIO | IRQ(11)
[rtl8139] BAR0=0xC000 (I/O), IRQ=11, MAC=52:54:00:12:34:56
```

---

## 2. Why Option B (Ring 3) and not Option A (Ring-0 modules)?

Two fundamentally different driver models were on the table:

| | **Option A — Ring-0 modules** (Linux `.ko` style) | **Option B — Ring-3 drivers** (microkernel style) |
|---|---|---|
| Privilege level | Ring 0, shared kernel address space | Ring 3, separate address space per driver |
| Hardware access | direct, unrestricted | mediated via gated syscalls |
| Isolation | **none** — bug = kernel panic | **full** — bug = only the driver process dies |
| Performance | maximal | context-switch overhead per IRQ/operation |
| Binary format | requires ELF `ET_REL` | flat binary (existing pipeline) |
| Loader complexity | high | low |
| Precedent | Linux, Windows | QNX, Minix 3, seL4, Fuchsia |

### The decisive reasons for Option B

**1. No ELF loader needed — the existing `exec` pipeline is enough.**
A Ring-0 module would have to be loaded into the **shared** kernel address space.
That means it cannot have a fixed link address (collisions with other modules)
→ it would need **relocation processing**. It would also call kernel functions
directly by their address → it would need an **exported symbol table + runtime
linking**. Both require a full ELF `ET_REL` parser, which KAOS does not have today
(see `docs/elf.md`, where dynamic linking is explicitly listed as "out of scope").

A Ring-3 driver, by contrast:
- is linked like any other user program to the **fixed address** `USER_CODE_BASE`
  (`0x0000_7000_0000_0000`, `process/types.rs:8`)
  (`relocation-model=static` + `objcopy -O binary`),
- gets its **own address space (CR3)** via `process/loader.rs:109`, so that all
  drivers can live at the same address without colliding,
- calls kernel services exclusively via `int 0x80` + a **syscall number** — a
  constant, not a symbol that needs resolving.

→ **Relocation and symbol linking disappear entirely.** The existing
`exec_from_vfs()` pipeline (`process/loader.rs:86`) loads a driver unchanged.

**2. Isolation as an architectural principle.**
A faulty driver (null deref, off-by-one) causes a page fault in Ring 3 that
terminates **only the driver process** — the kernel and all other drivers keep
running. In Ring 0 the same bug would be a system panic. This is the main reason
microkernel systems (QNX, Minix 3) are used in safety-critical domains.

**3. It fits the existing architecture.**
KAOS already has a mature Ring-3 model: a syscall ABI (`int 0x80`,
`syscall/dispatch/mod.rs`), per-task address spaces, `exec`, `wait`, and even PCI
query syscalls (nr. 23/24). Option B builds on this additively, instead of
introducing a second, parallel load/link system.

### The accepted trade-off

Option B costs **performance** (a context switch per IRQ and per hardware
access). For high-throughput devices (10GbE, NVMe) Ring 0 would be faster. The
per-*access* part of this cost can moreover be largely eliminated without leaving
Ring 3 — see the direct-access model in section 3.6. For
KAOS as a teaching/design-oriented kernel with devices like PS/2, serial ports,
older NICs (RTL8139) and ATA, isolation and simplicity clearly win. Should real
performance demand arise later, an individual driver can be pulled into Ring 0
without abandoning the overall model.

---

## 3. The gated design: how a Ring-3 program gets *no* full hardware access

The obvious objection to Option B is: *"If there are syscalls for port I/O, then
doesn't every program have direct hardware access again?"*

The answer is **capability gating**. "Isolation" here does not mean *"no hardware
access"* (a driver *must* reach its hardware), but rather:

1. **Least privilege** — a driver can only reach the ports/memory regions/IRQs of
   its *own* device, nothing else.
2. **Mediated access** — every access goes through the kernel, which can validate,
   restrict, and revoke it.
3. **Fault containment** — a crash stays inside the process.

### 3.1 Per-task capabilities

Every task gets a capability set. A normal program (`hello`, `shell`) has an
**empty** set and receives `PermissionDenied` on every privileged syscall. Only
tasks spawned as drivers by the driver manager get specifically granted
capabilities.

```rust
// kernel/src/process/capabilities.rs (new)

bitflags::bitflags! {
    /// Privileges a task may hold for hardware access.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Capabilities: u32 {
        /// May call `IoPortRead`/`IoPortWrite` — but only on allowed ports.
        const IO_PORTS = 1 << 0;
        /// May call `MapPhysical` — but only on allowed physical regions.
        const MMIO     = 1 << 1;
        /// May call `IrqSubscribe`/`IrqWait` — but only on allowed vectors.
        const IRQ      = 1 << 2;
        /// May spawn other drivers (driver manager only).
        const SPAWN_DRIVER = 1 << 3;
    }
}

/// Fine-grained grants: *which* concrete resources a task may touch.
/// Checked in addition to the coarse capability flag.
#[derive(Default)]
pub struct ResourceGrants {
    /// Allowed I/O port ranges (inclusive): (start, end).
    pub io_ports: Vec<(u16, u16)>,
    /// Allowed physical MMIO regions: (phys_start, len).
    pub mmio_regions: Vec<(u64, u64)>,
    /// Allowed IRQ vectors.
    pub irqs: Vec<u8>,
}
```

> **Important:** The `Capabilities` flags are only the coarse level (*"may it do
> port I/O at all?"*). The actual security comes from the **`ResourceGrants`**
> (*"may touch exactly ports 0xC000–0xC0FF, nothing else"*). An RTL8139 driver
> that tries to access the ATA controller (port 0x1F0) fails the grant check even
> though it holds `IO_PORTS`.

### 3.2 Anchoring in the task

`TaskEntry` (`scheduler/roundrobin/types.rs:83`) is extended with a pointer to the
capability/grant set. Since `TaskEntry: Copy` (see the comment on `fpu_state`,
line 141), a **raw pointer to a heap-allocated block** is used — just like the FPU
state — not a `Box`:

```rust
// Addition to TaskEntry:
/// Hardware capabilities + resource grants of this task.
/// `null` for normal (unprivileged) programs.
/// Allocated at driver spawn, freed in `remove_task`.
pub caps: *mut DriverCaps,   // { Capabilities, ResourceGrants }
```

### 3.3 The gate check in the syscall path

Every privileged syscall starts with the same two-stage check:

```rust
// kernel/src/syscall/dispatch/driver.rs (new)

fn syscall_io_port_write_impl(port: u16, width: u8, value: u32) -> SyscallResult<u64> {
    // Stage 1: coarse capability — does the task hold IO_PORTS at all?
    let caps = current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    if !caps.flags.contains(Capabilities::IO_PORTS) {
        return Err(SyscallError::PermissionDenied);
    }
    // Stage 2: fine-grained grant — is EXACTLY this port allowed?
    if !caps.grants.io_ports.iter().any(|&(lo, hi)| port >= lo && port <= hi) {
        return Err(SyscallError::PermissionDenied);
    }
    // Only now: the actual hardware access (kernel performs it, in Ring 0).
    unsafe {
        match width {
            1 => PortByte::new(port).write(value as u8),
            2 => PortWord::new(port).write(value as u16),
            4 => PortLong::new(port).write(value),
            _ => return Err(SyscallError::InvalidArg),
        }
    }
    Ok(SYSCALL_OK)
}
```

This resolves the apparent contradiction: the CPU instructions `in`/`out` are
still executed **exclusively in Ring 0** (inside the kernel). The driver only
asks — and the kernel only complies if both the capability **and** the grant
match.

### 3.4 Comparison of actual privileges

| | Ring-3 driver (with grants) | Ring-0 module |
|---|---|---|
| Port access | only grant-allowed ports | all ports |
| Memory | only own address space + grant-allowed MMIO | all physical/kernel memory |
| `cli`/`lgdt`/`wrmsr`/CR3 write | impossible (Ring 3 → #GP) | freely available |
| Crash | only the process dies | kernel panic |
| Runtime revocation | yes (revoke grant/mapping) | no |

### 3.5 Limits — an honest assessment

- A **malicious** authorized driver can abuse "its" hardware. In particular, a
  DMA-capable device can, without an **IOMMU**, read/write arbitrary physical
  memory and thereby bypass address-space isolation. Protection against this
  requires an IOMMU (VT-d) — outside the scope of this plan.
- The gating reliably protects against the **realistic case**: a driver with a
  *bug*. Errors stay local, the rest of the system remains intact.

### 3.6 Alternative enforcement: direct hardware access (IOPB + MMIO mapping)

The syscall-mediated model of section 3.3 puts the kernel on the path of *every*
port access (`int 0x80` per `in`/`out`). That is simple and maximally mediated,
but it is also the source of Option B's per-access cost (section 2). x86 offers a
second enforcement model that keeps the **isolation and least-privilege**
properties of section 3 while removing the per-access context switch: let the
driver touch *only its own* hardware **directly** in Ring 3, with the **hardware**
(MMU + TSS) enforcing the grant instead of a kernel check per access.

This is the same split Linux uses for its user-space driver frameworks
(UIO/VFIO, and `ioperm()` for ports) — see the precedent note at the end of this
section.

**MMIO registers — map once, then access directly.**
`MapPhysical` (syscall 31, section 5) already does exactly this: the BAR is mapped
page-by-page into the driver address space (`USER_MMIO_BASE`, NX+PCD). Afterwards
the driver performs `read_volatile`/`write_volatile` **directly in Ring 3 — no
syscall per access**. The page tables enforce the grant: touching an unmapped
address faults (#PF) and the fault is contained in the process. So for memory-BAR
devices the direct model *is* already the plan — there is no `IoPortRead/Write`
equivalent on the hot path.

**Port registers — the I/O Permission Bitmap (IOPB).**
The port-I/O analogue is the **IOPB in the TSS** (together with the IOPL field).
By setting the bits for a driver's granted ports in a per-task I/O bitmap, the CPU
allows that Ring-3 task to execute `in`/`out` on **exactly those ports** directly;
any other port raises #GP (contained in the process). This gives the same
granularity as `ResourceGrants.io_ports` — but enforced by hardware, with **no
syscall per access** — and it would make syscalls 29/30 unnecessary.

> Current state: the IOPB is deliberately **disabled** today. `arch/gdt.rs:273`
> sets `io_map_base` beyond the TSS limit ("Disable I/O bitmap … No per-port
> permission bitmap is active"), and there is a single shared TSS (single-core,
> no SMP).

The two models are symmetric:

| Resource | Mediated (syscalls 29/30) | Direct (this section) | Enforced by |
|---|---|---|---|
| MMIO register | — (already mapped) | map BAR, then `read/write_volatile` | page tables (#PF) |
| Port register | `int 0x80` per access | `in`/`out` after IOPB grant | TSS IOPB (#GP) |
| Grant granularity | per-access check | per-resource, set at spawn | hardware |
| Per-access cost | one syscall | none | — |

**What the direct model gives up.**
Direct access weakens point 2 of section 3 (*"mediated access — validate,
restrict, revoke"*). The kernel only mediates at **grant time**, not per access:
- no per-access validation or logging,
- **revocation becomes coarse**: revoking a port grant means rewriting the IOPB +
  reloading the TSS; revoking an MMIO grant means unmapping pages + a TLB flush —
  not a cheap per-call `PermissionDenied`.

**Plumbing cost of the IOPB.**
Full per-port granularity is an **8 KiB bitmap (65536 bits) per task**. With a
single shared TSS the bitmap must be **rewritten on every context switch** into a
driver task (or a per-task TSS must be introduced). By contrast the syscall path
(section 3.3) needs **no new CPU structures** — it reuses `int 0x80`.

**It does not help the IRQ path.**
The dominant cost for an interrupt-driven driver is **waking the Ring-3 task per
IRQ** (the bridge, syscalls 33–35), not the register poke. Direct access removes
the per-register syscall but leaves the per-IRQ context switch untouched. For a
low-traffic device like the COM2 PoC (a few bytes per IRQ) the per-access syscall
is negligible against the IRQ wake.

**Recommendation.**
- Keep the **mediated port-I/O syscalls (29/30)** for the COM2 PoC and other
  low-traffic, port-only devices: simplest, fully mediated, zero new CPU plumbing.
- Use **MMIO-direct mapping (syscall 31)** wherever a device exposes a memory BAR
  — already the plan, and the right fast path.
- Treat the **IOPB** as a *later* optimization, justified only once a
  high-throughput, port-only device makes the per-access syscall a measured
  bottleneck. At that point it can replace 29/30 with hardware-enforced grants.

**Linux precedent.** Mainline Linux drivers run in Ring 0, so they access both
MMIO (`ioremap` + `readl/writel`) and ports (`inb/outb`) directly — at the price
of zero isolation (a driver bug panics the kernel = Option A). Linux's *isolated*
user-space drivers use exactly the mechanisms above: **UIO** maps the BAR and
delivers IRQs via a file descriptor; **VFIO** adds the IOMMU for safe DMA (the gap
of section 3.5); **`ioperm()`** sets the TSS IOPB for direct port access from
Ring 3. KAOS Option B is therefore closest to UIO/VFIO, not to a mainline `.ko`
driver.

---

## 4. Current state: what exists, what is missing

### Already present (building blocks)
- Reading a file from kernel context: `vfs::read_file() -> Vec<u8>` (`io/vfs.rs`)
- Loading + starting a program: `exec_from_vfs()` (`process/loader.rs:86`), syscall 17
- A separate address space per task (CR3 clone, `process/loader.rs:109`)
- NX-/W^X-capable page flags, EFER.NXE enabled (`memory/vmm/page_table.rs:15`)
- Frame allocation + mapping (`alloc_frame_phys()`, `map_user_page()` `mapping.rs:677`)
- Dynamic IRQ registration: `register_irq_handler(vector, handler)` (`arch/interrupts/mod.rs:135`),
  `IrqHandler = fn(u8, &mut SavedRegisters) -> *mut SavedRegisters` (`types.rs:71`)
- Free IRQ vectors: IRQ10, IRQ11 (`arch/interrupts/types.rs:16-17`)
- PCI enumeration + query incl. BARs/IRQ line (`drivers/pci/mod.rs`, syscalls 23/24)
- User library `lib_kaos` with syscall wrappers (`lib_kaos/src/`)

### Missing (to be built)
- A per-task capability/grant system (section 3)
- Privileged syscalls: port I/O, MMIO mapping, IRQ subscribe/wait (section 5)
- An IRQ→user event bridge (kernel handler wakes the blocked driver task)
- A driver manager (spawns drivers, assigns grants from PCI data, matching)
- A `lib_driver` crate (user-side wrappers for the new syscalls)
- An example driver as proof of concept

---

## 5. New syscall ABI

Append to `SyscallId` (`syscall/types.rs:63`, next free number = **29**):

| Nr | Name | Args (RDI, RSI, RDX, R10) | Capability | Description |
|----|------|---------------------------|------------|-------------|
| 29 | `IoPortRead` | port, width | `IO_PORTS` + port grant | Reads 1/2/4 bytes from an I/O port |
| 30 | `IoPortWrite` | port, width, value | `IO_PORTS` + port grant | Writes 1/2/4 bytes to an I/O port |
| 31 | `MapPhysical` | phys_addr, len, flags | `MMIO` + MMIO grant | Maps a physical region (BAR) into the driver address space, returns the user VA |
| 32 | `UnmapPhysical` | user_va, len | `MMIO` | Removes an MMIO mapping |
| 33 | `IrqSubscribe` | vector | `IRQ` + IRQ grant | Subscribes an IRQ vector for this task |
| 34 | `IrqWait` | vector, timeout_ms | `IRQ` + IRQ grant | Blocks until the subscribed IRQ fires (or timeout) |
| 35 | `IrqAck` | vector | `IRQ` + IRQ grant | Acknowledges handling (manages the PIC EOI) |
| 36 | `SpawnDriver` | name_ptr, caps, grants_ptr | `SPAWN_DRIVER` | Loads+spawns a driver with defined grants (driver manager only) |

> **Note on `IoPortRead`/`IoPortWrite` (29/30):** these mediate *every* port
> access through `int 0x80`. They are the right choice for the COM2 PoC and other
> low-traffic, port-only devices (simple, fully mediated, zero new CPU plumbing).
> For a high-throughput, port-only device they can later be **replaced by the TSS
> I/O Permission Bitmap (IOPB)**, which grants direct, hardware-enforced `in`/`out`
> on exactly the allowed ports with no syscall per access — see section 3.6 for
> the trade-offs. MMIO already takes the direct path via `MapPhysical` (31), so
> 29/30 are only needed for port-mapped registers.

To be extended alongside:
- The `SyscallId` enum + `*_u64` constants (`syscall/types.rs`)
- `syscall_name_for_number()` + `dispatch_checked()` (`syscall/dispatch/mod.rs:45,97`)
- A new `SyscallError::PermissionDenied` variant + ABI sentinel
  (`SYSCALL_ERR_PERMISSION = u64::MAX - 4`) in `syscall/types.rs`

### MMIO mapping (syscall 31) in detail

`MapPhysical` is security-critical because it brings physical memory into the
user address space. Flow:

1. Capability `MMIO` + grant check: does `[phys_addr, phys_addr+len)` lie
   entirely within an allowed `mmio_regions` entry?
2. Pick a free user-VA range above the user heap (a new region, e.g.
   `USER_MMIO_BASE`, see `vmm_constants.rs`).
3. Map page by page, analogous to `map_user_page()`, but with flags
   `present | writable | user | no_execute` and **cache-disable (PCD)** for MMIO.
4. Return the user VA.

> The grant ensures a driver can only map the **BAR of its own device** — which
> the driver manager entered as a grant at spawn time from the PCI data
> (`UserPciDevice.bars`, `syscall/types.rs:376`).

### The IRQ bridge (syscalls 33–35) in detail

The core concept that lets Ring-3 drivers react to hardware interrupts:

1. **Subscribe (`IrqSubscribe`)**: The kernel registers a generic
   `driver_irq_trampoline` via `register_irq_handler(vector, ...)` and records
   `vector -> task_id`.
2. **Wait (`IrqWait`)**: The driver task blocks (scheduler state `Blocked`, cf.
   `TaskState`, `types.rs:56`) on a wait queue bound to the vector (the existing
   `WaitQueue` primitives can be used).
3. **IRQ fires**: The generic kernel handler (Ring 0) runs in the top half, sets
   a "pending" flag and **wakes** the waiting task. It does **not** yet send an
   EOI to the PIC (the device is only serviced by the user driver).
4. **Handling**: The woken driver reads device registers (via the port-I/O or
   MMIO syscalls) and processes the interrupt.
5. **Acknowledge (`IrqAck`)**: The driver signals completion; the kernel sends the
   PIC EOI.

> This is exactly where Option B's performance cost lies (multiple context
> switches per IRQ) — deliberately accepted (section 2).

---

## 6. Driver manager and device binding

A privileged user-space process (or initially a kernel routine) that loads
drivers and assigns them their grants.

```
1. Enumerate PCI devices (present: pci::get_devices / syscalls 23/24).
2. For each device: find the matching driver (matching table vendor:device → file).
3. Start the driver via SpawnDriver(name, caps, grants), where grants are
   derived from the device's PCI data:
     - io_ports     := from BARs with bar_type == Io
     - mmio_regions := from BARs with bar_type == Memory32/64
     - irqs         := [device.interrupt_line]
4. The driver reports "bound" on success.
```

Matching table (static at first, later read from a config file on disk):

```rust
// (vendor_id, device_id, driver_file)
const DRIVER_DB: &[(u16, u16, &str)] = &[
    (0x10EC, 0x8139, "RTL8139.DRV"),   // Realtek RTL8139 NIC
    (0x8086, 0x100E, "E1000.DRV"),     // Intel 82540EM NIC
];
```

> This directly addresses the existing but so far unused gap: the PCI layer finds
> devices, but nobody binds drivers to them. The driver manager closes that gap.

---

## 7. User-side library: `lib_driver`

Analogous to `lib_kaos`, a crate with safe wrappers around the new syscalls so
drivers can be written idiomatically:

```rust
// lib_driver/src/io.rs
pub fn inb(port: u16) -> u8  { /* syscall IoPortRead, width=1 */ }
pub fn outb(port: u16, v: u8) { /* syscall IoPortWrite, width=1 */ }
pub fn inw(port: u16) -> u16 { /* ... */ }
pub fn outw(port: u16, v: u16) { /* ... */ }

// lib_driver/src/mmio.rs
pub struct Mmio { base: *mut u8, len: usize }
impl Mmio {
    pub fn map(phys: u64, len: usize) -> Result<Self, SysError> { /* MapPhysical */ }
    pub fn read32(&self, off: usize) -> u32 { /* volatile read */ }
    pub fn write32(&self, off: usize, v: u32) { /* volatile write */ }
}

// lib_driver/src/irq.rs
pub fn subscribe(vector: u8) -> Result<(), SysError>;
pub fn wait(vector: u8, timeout_ms: u32) -> Result<(), SysError>;
pub fn ack(vector: u8) -> Result<(), SysError>;
```

A driver `main` then looks like this:

```rust
#![no_std]
#![no_main]
use lib_driver::{io::*, irq, mmio::Mmio};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // BAR/IRQ are provided by the driver manager as grants and passed to the
    // driver as arguments/env.
    let io_base = 0xC000u16;
    irq::subscribe(11).unwrap();
    // ... device init via inb/outb(io_base + reg) ...
    loop {
        irq::wait(11, 0).unwrap();
        // ... service the interrupt ...
        irq::ack(11).unwrap();
    }
}
```

---

## 8. Implementation phases

Each phase is independently testable.

### Phase 0 — Driver trait for *existing* drivers (refactoring, ring 0)
An optional cleanup step, independent of loading: introduce a `Driver` trait and
a registry and migrate ATA/keyboard/serial onto it. This creates a clean internal
structure but does not yet change the loading model.

### Phase 1 — Capabilities (foundation)
- `process/capabilities.rs`: `Capabilities`, `ResourceGrants`, `DriverCaps`.
- `TaskEntry.caps` pointer + allocation/freeing (analogous to `fpu_state`).
- A `current_task_caps()` helper.
- `SyscallError::PermissionDenied` + ABI sentinel.
- **Test:** a normal task has empty caps; the helper returns the correct `None`/set.

### Phase 2 — Port-I/O syscalls (29/30)
- `syscall/dispatch/driver.rs` with the gate check (section 3.3).
- Registration in `dispatch_checked` + `syscall_name_for_number`.
- Create the `lib_driver` crate, `inb/outb/inw/outw`.
- **Test:** a task with a port grant may access; without a grant → `PermissionDenied`;
  access outside the granted range → `PermissionDenied`.

### Phase 3 — MMIO mapping (31/32)
- A `USER_MMIO_BASE` region in `vmm_constants.rs`.
- `MapPhysical`/`UnmapPhysical` with grant check, NX+PCD flags.
- `lib_driver::mmio`.
- **Test:** a grant-allowed physical region becomes mappable and read/writable; a
  disallowed region → `PermissionDenied`.

### Phase 4 — IRQ bridge (33–35)
- A generic `driver_irq_trampoline`, a `vector -> task_id` table.
- A wait queue per vector; `IrqSubscribe/IrqWait/IrqAck`.
- PIC EOI management deferred until `IrqAck`.
- `lib_driver::irq`.
- **Test:** a software-triggerable IRQ (e.g. an IRQ on a free vector) deterministically
  wakes the waiting task.

### Phase 5 — Driver manager + `SpawnDriver` (36)
- Grant derivation from PCI BARs/IRQ line.
- A matching table + `auto_probe`.
- The `SpawnDriver` syscall (only with `SPAWN_DRIVER`).
- **Test:** the manager spawns a driver with correctly derived grants.

### Phase 6 — Example driver (end-to-end proof)

**Primary proof-of-concept: a COM2 serial driver** (16550 UART, ports
0x2F8–0x2FF, IRQ3). This is the recommended *first* driver because it proves the
novel and risky parts of the framework — capability-gated port I/O (syscalls
29/30) **and** the IRQ→user bridge (syscalls 33–35) — while avoiding everything
that would complicate a first proof:

- **No MMIO and no DMA** → Phase 3 is not required, and the IOMMU gap of
  section 3.5 is not touched. A UART is pure port I/O.
- **No resource conflict** → COM1 (0x3F8/IRQ4) is the kernel's debug serial;
  COM2 (0x2F8/IRQ3) is unused. The vector `IRQ3_COM2_VECTOR` already exists
  (`arch/interrupts/types.rs:9`). (By contrast, a PS/2 mouse would share the 8042
  controller ports 0x60/0x64 with the existing in-kernel keyboard driver — a
  resource conflict best avoided for a *first* PoC.)
- **Register logic already available** → can be cribbed almost 1:1 from the
  existing in-kernel `drivers/serial.rs`.
- **Trivially testable in QEMU** → attach COM2 to stdio/pty via `-serial`; bytes
  typed in wake the driver via IRQ3, which echoes them back.

Grants for this driver: `io_ports: [(0x2F8, 0x2FF)]`, `irqs: [IRQ3]`.

Recommended sub-steps:
1. **TX only** (needs phases 1+2): the driver writes a banner to COM2 — proves
   gating + port I/O without IRQs.
2. **RX with IRQ** (needs phase 4): an echo loop — proves the IRQ bridge.

- **Test:** `SERIAL2.DRV` is loaded from FAT32 at runtime, spawned by the driver
  manager with the grants above, subscribes to IRQ3, blocks in `irq::wait`, is
  woken on incoming bytes, reads them via `inb(0x2F8)` and writes them back via
  `outb` — while the kernel and all other tasks keep running normally.

**Later expansion stages** (once the serial PoC works): a **PS/2 mouse driver**
(IRQ12, pure port I/O — requires coordinating port 0x60/0x64 access with the
keyboard driver), and **RTL8139** (`-device rtl8139` in QEMU) as the first driver
that exercises MMIO **and** DMA (and thus motivates the IOMMU work).

---

## 9. Build integration

Drivers are built like user programs (`helper_build_user_programs.sh`):
`cargo build --target x86_64-unknown-none` → `objcopy -O binary` → copy `*.DRV`
to the FAT32 disk. Because of FAT32 8.3, driver files must live flat in the root
and have short names (e.g. `RTL8139.DRV`).

---

## 10. Open items / deliberately out of scope

- **IOMMU/DMA protection:** Without an IOMMU, a DMA-capable driver can bypass
  address-space isolation (section 3.5). Can be retrofitted later.
- **MSI/MSI-X:** Legacy PIC IRQs only at first (pin-based). MSI later.
- **Driver restart/recovery:** Automatic restart of a crashed driver by the
  driver manager — desirable, but later.
- **BSS in flat binaries:** The loader only zeroes the rest of the last page
  (`process/loader.rs:224`). Drivers with large static buffers must account for
  this (or use the heap via `mmap`).
- **Hot-plug:** Currently only boot-time enumeration; dynamic PCI hot-add later.
</content>
