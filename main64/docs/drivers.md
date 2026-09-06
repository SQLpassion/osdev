# User-Space Drivers in KAOS: Background Services, and How Applications Talk to Them

This document explains KAOS's user-space driver architecture from first principles, and — this is the part that has changed the most recently — how an ordinary application actually talks to a driver once that driver is running. Its purpose is not merely to describe **what** the code does, but to explain **why** each layer exists and how data, permissions, and hardware events move through the system.

The architecture went through an important evolution that this revision of the document reflects. Originally, a NIC driver was launched directly from the shell, which then blocked until the driver process exited — the shell and the driver shared a single, synchronous, foreground relationship, much like running `ls` in a Unix terminal and waiting for it to return. That model is gone. A KAOS NIC driver today is a permanent background service: once started, it registers itself under a well-known name, then loops forever, and no other process ever waits for it to finish. Applications that want to use the network — `net-tools.bin` is the concrete example this document follows end to end — locate the driver by name while it is already running, and exchange data with it across the Ring 3/Ring 3 process boundary using a small, dedicated set of kernel syscalls that copy raw Ethernet frames back and forth. Understanding *that* mechanism — how two independent, mutually distrusting processes cooperate to move packets without ever sharing memory — is the main new content in this revision.

The concrete hardware example throughout is the Realtek RTL8139 network driver, one of two NIC drivers that exist in this codebase today (the other being an Intel Gigabit Ethernet driver covering the 82577LM/I219-V family). Both are ordinary Ring 3 processes that retain controlled access to PCI hardware, MMIO registers, and DMA memory, and both share almost all of their infrastructure through a common runtime crate. The protocol stack the drivers hand frames to — Ethernet, ARP, IPv4, and ICMP — is documented separately in [`docs/networking.md`](networking.md), since it is hardware-agnostic and lives in its own crate, `lib_net`. That document's §9 in particular already covers the client-side view of the app↔driver conversation in detail; this document approaches the same conversation from the driver-lifecycle and kernel-mechanism side, and the two are meant to be read together.

The most important implementation files are:

* [`kernel/src/process/capabilities.rs`](../kernel/src/process/capabilities.rs) — capability bits and resource grants
* [`kernel/src/drivers/driver_db.rs`](../kernel/src/drivers/driver_db.rs) — PCI-device-to-driver-name binding and grant derivation
* [`kernel/src/drivers/registry.rs`](../kernel/src/drivers/registry.rs) — the name registry and per-driver packet rings
* [`kernel/src/syscall/dispatch/driver.rs`](../kernel/src/syscall/dispatch/driver.rs) — every driver-related syscall implementation
* [`lib_driver`](../lib_driver/src/lib.rs) — the user-space wrapper crate (`Mmio`, `Dma`, `drv`, `client`)
* [`lib_driver_runtime`](../lib_driver_runtime/src/lib.rs) — PCI discovery helpers and the shared background event loop
* [`drivers/rtl8139`](../drivers/rtl8139/src/rtl8139.rs) and [`drivers/intel_nic`](../drivers/intel_nic/src/intel_nic.rs) — the two concrete driver binaries
* [`user_programs/drivers`](../user_programs/drivers/src/main.rs) — `DRIVERS.BIN`, the driver lifecycle manager
* [`user_programs/net_tools`](../user_programs/net_tools/src/main.rs) — `NETTOOLS.BIN`, the application example this document follows

---

## 1. Why a Driver Needs Special Privileges

An ordinary application works with abstractions. It opens a file, writes text to a console, or asks the operating system for memory. It does not need to know which PCI bus contains a storage controller or which register bit starts a transmission. A device driver sits exactly at that boundary. It translates an abstract request such as "send this Ethernet frame" into register accesses and memory operations understood by a particular device.

This work requires capabilities that normal processes must not possess. A network driver has to read and write the registers of a PCI card. It has to prepare memory that both the CPU and the device can access. It may also have to react to asynchronous hardware interrupts. If every process could perform these operations without restriction, a bug or malicious program could reconfigure unrelated devices, overwrite foreign memory, or freeze the entire machine.

x86 therefore separates execution into privilege levels. KAOS, like many operating systems, primarily uses Ring 0 and Ring 3. The kernel executes in Ring 0 and may run privileged CPU instructions, modify page tables, and program interrupt controllers. Applications execute in Ring 3. When they attempt a forbidden operation, the CPU raises an exception instead of carrying it out.

A traditional monolithic kernel also runs drivers in Ring 0. This is fast, but a bad pointer inside a driver can corrupt the kernel. KAOS chooses a different architecture: every NIC driver is a normal process with its own virtual address space, and — this is the part that makes it feel unlike a typical user-space program — one that never exits under normal operation. It is closer in spirit to a Unix daemon than to a command a user runs and waits for. The kernel retains ownership of privileged mechanisms and exposes only small, validated interfaces; the driver, once started, keeps running as an independent scheduled task for as long as the machine is up, quietly serving requests from whichever applications come and go around it.

The resulting model can be summarized as follows:

```text
Ring 3

  DRIVERS.BIN                          RTL8139 driver task
  loads the driver once,               registers as "nic:rtl8139",
  then returns to its own prompt        then loops forever
             |                                  |
             | SpawnDriver                      | MapPhysical / AllocDma
             | plus exact grants                | DrvRegister
             | (does not wait)                   | NetRecv / NetSend / DrvPublishStatus
             v                                  v
---------------------------- syscall boundary ----------------------------
Ring 0

  capability checks -> paging -> PMM -> hardware
  driver registry (name -> task id, packet rings, status snapshot)

             ^
             | DrvLookup / NetSend / NetRecv / DrvQuery
             |
Ring 3       |
  net-tools.bin — an ordinary application with no hardware
  access at all, talking to the driver purely through the registry
```

The essential idea is that a driver does not receive unrestricted "hardware access." It receives authority over the exact resources belonging to its device — and, once running, it becomes an addressable service that any other process can find by name and exchange packets with, without either side ever seeing the other's memory.

---

## 2. The Four Hardware Concepts Behind the Design

Three concepts carry almost the entire hardware-facing half of the implementation: PCI, BAR/MMIO, and DMA. They describe different aspects of communication between the CPU and a device, and they are identical regardless of which of the two concrete drivers you are reading. A fourth concept, IRQs, is covered briefly below for background — the kernel uses it for its own devices (the timer, the keyboard, the primary ATA controller) — but neither NIC driver in this codebase uses it at all; §9 explains why.

### 2.1 PCI and Device Identity

PCI is a standardized bus for attaching devices. Every PCI device exposes a configuration space containing, among other fields, a vendor identifier, a device identifier, a device class, interrupt information, and Base Address Registers. The RTL8139 is recognized by the combination of vendor ID `0x10EC` and device ID `0x8139`; the Intel Gigabit driver recognizes four device IDs under vendor `0x8086` (`0x10EA`, `0x15B8`, `0x10D3`, `0x100E`), covering different silicon revisions of the same family.

A PCI function is addressed by a bus, device, and function tuple. For example, `00:03.0` means bus 0, device 3, function 0. KAOS scans these addresses during boot and stores the devices it finds. User-space code later queries that cached list through the existing PCI syscalls — both the driver binaries themselves (to find their own device) and `DRIVERS.BIN` (to decide whether it is even worth attempting to load a given driver) do this same PCI enumeration independently.

During enumeration, the kernel enables three bits in every discovered device's PCI Command Register once that device is actually reserved for a driver:

```rust
let orig_cmd = unsafe { pci_config_read(bus, slot, func, 0x04) };
let new_cmd = (orig_cmd & 0xFFFF_0000)
    | ((orig_cmd & 0x0000_FFFF) | 0x0007);
unsafe { pci_config_write(bus, slot, func, 0x04, new_cmd) };
```

The lowest three bits enable I/O Space, Memory Space, and Bus Mastering. Memory Space is necessary before the device responds to MMIO accesses. Bus Mastering allows the device to initiate DMA transactions. The existing [`docs/pci.md`](pci.md) provides a deeper explanation of PCI configuration-space access and BAR discovery.

### 2.2 BARs and MMIO

A Base Address Register, or BAR, describes an address range provided by a device. With a memory BAR, the range looks like memory from the CPU's perspective, but it is not ordinary RAM. A read may return current device state, and a write may initiate a hardware operation.

This mechanism is called Memory-Mapped I/O, or MMIO. The RTL8139 exposes registers for its MAC address, receive buffer, interrupt state, and transmit buffers. Their offsets are defined in [`rtl8139.rs`](../drivers/rtl8139/src/rtl8139.rs):

```rust
pub const REG_MAC0: usize = 0x00;
pub const REG_TSD0: usize = 0x10;
pub const REG_TSAD0: usize = 0x20;
pub const REG_RBSTART: usize = 0x30;
pub const REG_CHIPCMD: usize = 0x37;
pub const REG_IMR: usize = 0x3C;
pub const REG_ISR: usize = 0x3E;
pub const REG_RCR: usize = 0x44;
```

If the physical BAR begins at `0xFEB0_0000`, then `REG_CHIPCMD = 0x37` means that the command register is physically accessible at `0xFEB0_0037`. A Ring 3 process cannot dereference that physical address directly. The kernel must first map it into the process's virtual address space under controlled conditions, described in §7 below.

MMIO accesses must also be volatile. For ordinary memory, a compiler may assume that two reads return the same value if visible program code did not write between them. A hardware register can change independently. `read_volatile` and `write_volatile` tell the compiler that every individual access must really occur.

### 2.3 DMA

Direct Memory Access means that a device reads or writes main memory directly. To transmit a packet, the CPU places an Ethernet frame into RAM and tells the network card its physical address. The card reads that buffer and sends the bytes. During reception, the card writes incoming frames into a prepared RAM region.

This introduces an important distinction. A driver uses virtual addresses because each process owns a separate virtual address space. The PCI device in this system does not understand process page tables. It requires physical addresses. A DMA buffer therefore has two addresses:

```text
The driver sees:   virtual address  0x00007800_00001000
The page table:    maps that address to a RAM frame
The device sees:   physical address 0x00000000_01234000
```

The RTL8139 also requires physically contiguous memory. Several consecutive virtual pages can normally point to unrelated physical frames. The kernel therefore provides a PMM allocation routine that searches for a contiguous sequence of free physical frames, described in §8.

### 2.4 IRQs and EOI

An Interrupt Request, or IRQ, is an asynchronous signal from a device to the CPU. Instead of continuously inspecting a register, the CPU can perform other work and be interrupted when the device needs attention. The legacy PIC used by this system manages 16 IRQ lines, mapped to IDT vectors 32 through 47. After an interrupt has been serviced, software must issue an End Of Interrupt, or EOI, to tell the PIC that the line may be used again; the kernel's own interrupt dispatcher (`arch::interrupts::dispatch_irq`) handles this for every IRQ it services — the timer, the keyboard, and the primary ATA controller among them.

Neither NIC driver in this codebase uses interrupts at all, and the kernel exposes no syscall path that would let a Ring 3 driver subscribe to one. §9 explains why polling, not interrupt-driven I/O, is the design this codebase actually uses.

---

## 3. Two Drivers, One Shared Runtime

Before following a driver through its startup path, it is worth seeing the shape both concrete drivers share, because almost none of the code below is specific to the RTL8139. [`lib_net::NicDevice`](../lib_net/src/nic.rs) is the small trait that separates hardware-specific code from everything hardware-agnostic:

```rust
pub trait NicDevice {
    fn mac(&self) -> MacAddress;
    fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError>;
    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize>;
    fn shutdown(&mut self);
}
```

`Rtl8139Device` and the Intel driver's `IntelNicDevice` each implement exactly this trait, and nothing above it — the protocol stack in `lib_net`, the background event loop, and every syscall this document describes — cares which one it is talking to.

Two further pieces of infrastructure are shared through [`lib_driver_runtime`](../lib_driver_runtime/src/lib.rs), a crate that did not exist in the very first version of this feature and was extracted once the duplication between the two drivers became obvious. [`discovery.rs`](../lib_driver_runtime/src/discovery.rs) contains `find_bound_device()`, which asks the kernel — via the `DrvBoundDevice` syscall (§6) — for the exact PCI device `SpawnDriver` already bound this task to (`driver_db::derive_grants`), and `map_mmio_bar()`, which picks the first memory-type BAR with a non-zero address (falling back to a caller-preferred index, since the RTL8139's data sheet places its usable BAR at index 1 rather than 0) and maps it via `Mmio::map()`. Both driver binaries' `_start()` functions call these two functions and nothing else changes between them. An earlier version of `find_bound_device()`, `find_pci_device()`, independently re-scanned the PCI bus and re-matched each driver's own hardcoded copy of its supported `(vendor_id, device_id)` pairs — the same "two sources of truth" problem §4 describes `DrvProbe` solving for `DRIVERS.BIN`, just one layer further in, and one that could not be relied on to land on the same device `SpawnDriver` had reserved if more than one matching card were installed. `find_bound_device()` removes the driver's own copy of that table entirely; the Intel driver still keeps a small `(vendor_id, device_id) -> NicModel` table of its own in `main.rs`, but that is a different concern — hardware-quirk selection once a device is already known-bound, not device discovery. [`repl.rs`](../lib_driver_runtime/src/repl.rs) contains `run_background_driver()`, the permanent event loop described in full in §12 — this is the single function that turns a `NicDevice` implementation plus a `NetworkStack` into a running, addressable network service, and both drivers hand off to it as the very last thing their `_start()` function does.

The practical consequence for a reader is that everything in §7 through §12 below, illustrated with RTL8139-specific register names and offsets, applies to the Intel driver too, just with different constants; and everything from §12 onward applies identically to both, since it lives in shared code neither driver has its own copy of.

---

## 4. From `DRIVERS.BIN` to a Running Background Task

A NIC driver is not launched by typing its own name at the shell prompt, and the shell itself contains no driver-spawning logic at all anymore. Loading a driver is the job of a small, dedicated Ring 3 program, [`user_programs/drivers`](../user_programs/drivers/src/main.rs), which ships on the disk image as `DRIVERS.BIN` and presents its own tiny REPL once the user runs it from the shell. Its `load <name.drv>` command is the direct descendant of what used to be shell code — the file that implements it, [`load_driver.rs`](../user_programs/drivers/src/load_driver.rs), says so explicitly in its own header comment, and is worth reading once because it explains *why* the logic had to move, not just that it did: `spawn_driver` requires the caller to hold `Capabilities::SPAWN_DRIVER`, and a driver-loading command only ever worked before because it happened to run inside the shell's own, already-privileged process. Once driver management became its own separate binary, that binary needed its own, narrowly delegated privilege — described in §21 — before `load` could work again at all.

`load_driver()` first asks the kernel whether attempting `SpawnDriver` is even worth it, via `lib_driver::spawn::probe_driver()` — a thin wrapper around a dedicated syscall, `DrvProbe` (§6), that answers exactly one question: is this binary name a known driver, and if so, is a matching PCI device currently present? An earlier version of this file kept its own copy of the binary-name-to-PCI-ID table right here, plus its own loop re-enumerating the PCI bus, purely to print a friendly, specific error message — "unknown driver name" versus "no matching PCI device found" are two different failure modes worth telling apart. That was two independent sources of truth for the same mapping (this file's copy, and the kernel's own `driver_db::DRIVER_DB`, described below) that had to be kept in sync by hand every time a driver was added. `DrvProbe` answers the question directly from the kernel's own table and its own cached PCI enumeration instead, so this file carries no PCI IDs of its own at all:

```rust
match lib_driver::spawn::probe_driver(file) {
    Err(_) => {
        println!("[drivers] Unknown driver '{}'.", file);
        return;
    }
    Ok(false) => {
        println!(
            "[drivers] Error: no matching PCI device found for '{}'.",
            file
        );
        return;
    }
    Ok(true) => {}
}
```

`DrvProbe`'s kernel-side implementation, `driver_db::device_present()`, asks the exact same question `derive_grants()` (described just below) answers internally while actually spawning a driver — matching the binary name against [`driver_db::DRIVER_DB`](../kernel/src/drivers/driver_db.rs) and then checking whether any of its supported PCI IDs appears in the kernel's cached device list — but without any of `derive_grants`'s side effects: no device reservation, no PCI Command Register writes. It is safe to call purely to ask "would this succeed?"

Once `probe_driver()` confirms it is worth attempting, `load_driver()` spawns the driver and, critically, does not wait for it:

```rust
let caps = 1; // MMIO (1)

// Step 3: spawn in the background -- no process::wait() call. `file` is
// passed through as-is: both `driver_db::lookup_driver` and the FAT32 VFS
// lookup `SpawnDriver` triggers are already case-insensitive, so no
// client-side canonicalization is needed.
match lib_driver::spawn::spawn_driver(file, caps, None) {
    Ok(tid) => {
        println!("[drivers] Driver '{}' started as TID {}", file, tid);
    }
    Err(err) => {
        println!("[drivers] Failed to load '{}': {:?}", file, err);
    }
}
```

The `drivers>` prompt returns immediately, the way a `list` or `help` command would. If the user then types `exit` inside `drivers.bin` to return to the shell, the driver keeps running exactly as before — `drivers.bin`'s own `exit` command terminates only its own REPL task; the driver it started earlier is a completely independent, unrelated scheduled task, and nothing about exiting `drivers.bin` touches it in any way. The only way to stop a running driver from inside KAOS is the explicit `unload <name>` command described in §20, which maps to a deliberate hard kill (§15).

`None` is passed as the resource-grant argument for the same reason it always was: an unprivileged caller could never be trusted to compute its own MMIO grant, since a wrong or dishonest value would just be a physical address the caller chose for itself. `derive_grants()`, in [`kernel/src/drivers/driver_db.rs`](../kernel/src/drivers/driver_db.rs), resolves the binary name against the kernel's own driver table, atomically reserves the matching PCI device against a second, concurrent `SpawnDriver` call for the same binary, enables that device's PCI Command Register bits, and only then builds the grant from the device's own BARs. Nothing about the grant ever originates in user space. §6 covers the exact syscall mechanics of `SpawnDriver` in full.

---

## 5. How Capabilities and Resource Grants Work Together

The security model is implemented in [`kernel/src/process/capabilities.rs`](../kernel/src/process/capabilities.rs). A capability describes a class of privileged operations. A resource grant identifies the concrete resource within that class.

```rust
pub struct Capabilities(u32);

impl Capabilities {
    pub const NONE: Self = Self(0);
    pub const MMIO: Self = Self(1 << 0);
    pub const SPAWN_DRIVER: Self = Self(1 << 2);
    pub const UNLOAD_DRIVER: Self = Self(1 << 3);
    pub const LIST_DRIVERS: Self = Self(1 << 4);
}

pub struct ResourceGrants {
    pub mmio_regions: Vec<(u64, u64)>,
    pub mmio_bump: u64,
}
```

This separation is fundamental. `MMIO` alone merely says that the process may map some physical resource. Without `mmio_regions`, the same driver could potentially map a framebuffer, APIC registers, or a storage controller. A concrete grant restricts it, for example, to `0xFEB0_0000..0xFEB0_0100`.

The kernel therefore checks both levels. The essential MMIO validation is:

```rust
let caps = scheduler::current_task_caps()
    .ok_or(SyscallError::PermissionDenied)?;

if !caps.flags.contains(Capabilities::MMIO) {
    return Err(SyscallError::PermissionDenied);
}

let grant_matched = caps.grants.mmio_regions.iter().any(|&(base, len)| {
    if let Some(grant_end) = base.checked_add(len) {
        phys_addr >= base && requested_end <= grant_end
    } else {
        false
    }
});
```

A partial overlap is insufficient. The entire requested physical range must fit inside one grant. `checked_add` prevents an integer overflow from wrapping a very large end address back into a small number.

`UNLOAD_DRIVER` and `LIST_DRIVERS` are a later addition to this same set, and their purpose is narrower and more specific than `MMIO`: they gate `DrvUnload` and enumeration of the driver registry respectively (§13, §20), and neither one is ever handed to a spawned driver itself — only to `DRIVERS.BIN`, by delegation, at `Exec` time (§21). `Capabilities::from_bits_truncate()` masks out any bit the caller didn't legitimately request, and a second mask, `driver_db::DRIVER_GRANTABLE_CAPS`, further restricts what `SpawnDriver` itself will ever attach to a *driver* task — that mask is exactly `MMIO`, so a driver process can never end up holding `SPAWN_DRIVER`, `UNLOAD_DRIVER`, or `LIST_DRIVERS` no matter what it asks for, closing off any possibility of a compromised driver spawning further drivers or unloading its siblings.

Capabilities belong to scheduler tasks. Because `TaskEntry` is copyable, it stores a raw pointer rather than a `Box<DriverCaps>`:

```rust
pub struct TaskEntry {
    // Register state, stack, FPU state, and other metadata...
    pub caps: *mut crate::process::capabilities::DriverCaps,
}
```

A normal task begins with a null pointer. `SpawnDriver` allocates the block on the heap, converts the `Box` with `Box::into_raw()`, and attaches the pointer to the new task. When the task is removed, `remove_task()` reconstructs exactly one `Box` and frees it. This ownership protocol is safety-critical: `Box::from_raw()` may only be called once for an allocation, so the pointer is nulled immediately after. It is also, as it turns out, the very same check that gates §13's driver-naming syscall: `DrvRegister` requires `scheduler::current_task_caps()` to return `Some`, i.e. it requires the caller to be a task that was spawned through this exact mechanism — an ordinary application, whose `caps` pointer is null, can never register a driver name.

---

## 6. The Syscall Boundary and ABI

Every driver-related syscall number is defined in one place, [`kernel/src/syscall/types.rs`](../kernel/src/syscall/types.rs). Kernel and user space must agree on these exact numeric values:

```rust
pub enum SyscallId {
    // Existing syscalls 0..29
    MapPhysical      = 30,
    UnmapPhysical    = 31,
    SpawnDriver      = 32,
    AllocDma         = 33,
    FreeDma          = 34,
    VirtToPhys       = 35,
    DrvRegister      = 36,
    DrvLookup        = 37,
    NetSend          = 38,
    NetRecv          = 39,
    DrvPublishStatus = 40,
    DrvQuery         = 41,
    DrvUnload        = 42,
    DrvList          = 43,
    DrvProbe         = 44,
    DrvBoundDevice   = 45,
}
```

The first six numbers (30–35) are the original hardware-access primitives this document covers in §7–§8: mapping BARs and managing DMA memory. The remaining ten (36–45) are the driver-naming and packet-transport layer that turns a running driver into an addressable service — §13 covers every one of them in detail, §4 covers `DrvProbe` in the context of `DRIVERS.BIN`'s `load` command, §20 covers the list/unload calls specifically in that same context, and §3 covers `DrvBoundDevice` in the context of a driver binary's own `_start()`.

[`lib_driver/src/raw.rs`](../lib_driver/src/raw.rs) provides small assembly stubs for one, two, three, or four parameters. A three-argument call is implemented as follows:

```rust
asm!(
    "int 0x80",
    inout("rax") ret,
    in("rdi") arg0,
    in("rsi") arg1,
    in("rdx") arg2,
    in("r10") 0u64,
);
```

`RAX` contains the syscall number on entry and the raw result on return. `RDI`, `RSI`, `RDX`, and optionally `R10` carry the arguments. `decode_result()` converts the raw result into `Result<u64, SysError>`.

The kernel's `dispatch_checked()` function in [`kernel/src/syscall/dispatch/mod.rs`](../kernel/src/syscall/dispatch/mod.rs) acts as a switchboard — it only selects a handler; every actual validation and security decision happens inside [`kernel/src/syscall/dispatch/driver.rs`](../kernel/src/syscall/dispatch/driver.rs).

`SpawnDriver` deserves one more detail beyond what §4 already described, because it addresses a subtle race a naive implementation would get wrong. The kernel does not load the new ELF image and immediately let the scheduler run it; it loads the image with `exec_from_vfs_blocked()`, which creates the task already in `TaskState::Blocked`. Only once parent linkage is established, the PCI device binding is confirmed (`driver_db::confirm_binding()`), and the `DriverCaps` block with its fully-derived grant is attached, does the kernel call `scheduler::unblock_task()` and let the new driver actually run. Without this, a timer interrupt landing between "task created" and "grants attached" could let the scheduler run a driver task that does not yet have the capabilities it needs — a race that would either crash the driver or, worse, be exploitable. If `confirm_binding()` itself fails (the device was concurrently claimed by another spawn), the whole operation is unwound and the task is never unblocked at all.

---

## 7. Mapping a Physical BAR into the Driver

Virtual memory gives each process a private view of addresses. Virtual address `0x1000` in two processes can refer to two unrelated physical frames. Page tables store this translation, and the CPU's `CR3` register identifies the root table of the current address space.

The kernel reserves a dedicated virtual window for driver mappings:

```rust
pub const USER_MMIO_BASE: u64 = 0x0000_7800_0000_0000;
```

This range sits above the ordinary user heap and below the stack guard. `classify_user_region()` recognizes it as `UserRegion::Mmio`, allowing the VMM to distinguish device memory from code, stack, heap, and guard pages.

Suppose a BAR begins at physical address `0xFEB0_0000`, but the requested register begins at `0xFEB0_0037`. Page tables operate on page boundaries, so the kernel separates the page base from the in-page offset:

```text
Physical address:        0xFEB0_0037
Page size:               0x1000
Aligned page base:       0xFEB0_0000
Offset inside the page:  0x0037
```

The implementation computes these values as follows:

```rust
let offset_in_page = phys_addr & (PAGE_SIZE_U64 - 1);
let page_phys_start = phys_addr & !(PAGE_SIZE_U64 - 1);
let page_phys_end = (end_phys + PAGE_SIZE_U64 - 1)
    & !(PAGE_SIZE_U64 - 1);
```

The per-task `mmio_bump` then reserves a consecutive virtual range. A bump allocator is the simplest possible address allocator: it begins at a base address and advances after every allocation. It does not reuse holes created by unmapping.

Each page is installed through `map_user_mmio_page()`. Missing lower page-table levels are created as necessary. The leaf page-table entry is present, writable, user-accessible, non-executable, cache-disabled, and write-through:

```rust
let entry = entry_ptr(pt, pt_idx);
(*entry).set_mapping(pfn, true, true, true);
(*entry).set_no_execute(true);
(*entry).set_pcd(true);
(*entry).set_pwt(true);
```

The non-executable bit prevents the process from treating device bytes as instructions. `PCD` and `PWT` create a strongly uncacheable mapping so that register operations reach the device. After changing an entry, `invlpg` invalidates a potentially stale TLB translation.

If any page cannot be mapped, the handler removes every page already created for this request. This rollback avoids exposing a partially completed mapping as though it were valid.

The returned pointer is `base_va + offset_in_page`, not necessarily the virtual page base. It therefore points to the exact originally requested register address. User space wraps it in [`lib_driver/src/mmio.rs`](../lib_driver/src/mmio.rs):

```rust
pub struct Mmio {
    base: *mut u8,
    len: usize,
}
```

A 16-bit read first checks its bounds and then performs a volatile access:

```rust
pub fn read16(&self, off: usize) -> u16 {
    assert!(off + 2 <= self.len, "MMIO read16 out of bounds");
    unsafe { core::ptr::read_volatile(self.base.add(off) as *const u16) }
}
```

Dropping `Mmio` invokes `UnmapPhysical`. The kernel removes only the page mappings; it must not return the physical BAR frames to the RAM allocator because those addresses belong to the device rather than allocated memory.

---

## 8. Allocating Physically Contiguous DMA Memory

The Physical Memory Manager tracks free frames in bitmaps. A clear bit represents an available 4 KiB frame. `alloc_contiguous_frames()` in [`kernel/src/memory/pmm/manager.rs`](../kernel/src/memory/pmm/manager.rs) scans each region and counts consecutive free bits. Its search can be understood schematically as follows:

```rust
let mut consecutive = 0usize;
let mut start_bit = 0u64;

for bit in 0..region.frames_total {
    if frame_is_free(bit) {
        if consecutive == 0 {
            start_bit = bit;
        }
        consecutive += 1;

        if consecutive == count {
            // Mark every frame and return the first PFN.
        }
    } else {
        consecutive = 0;
    }
}
```

When the desired run is found, the real implementation marks every bitmap bit as allocated, sets each reference count to one, and decreases the region's free-frame count. It returns the first Page Frame Number, or PFN. The physical address is `PFN * 4096`.

`AllocDma` requires at least one page and currently uses the `MMIO` capability as its authorization gate. After physical allocation, the kernel maps every frame consecutively into the driver's MMIO window. The driver receives the virtual address as the syscall result and the physical address through a validated writable user pointer.

[`lib_driver/src/dma.rs`](../lib_driver/src/dma.rs) keeps both addresses:

```rust
pub struct DmaBuffer {
    va: *mut u8,
    pa: u64,
    pages: usize,
}
```

`va()` is used by the CPU. `pa()` is written to the device. `as_slice()` and `as_mut_slice()` expose the mapped memory as Rust slices, and `Drop` invokes `FreeDma` when the object is actually destroyed.

The RTL8139 allocates two regions. The RX allocation contains four pages. The hardware ring itself is 8192 bytes, while the larger allocation leaves room for headers and wrap behavior. The TX pool contains two pages divided into four 2048-byte slots:

```rust
let rx_buffer = DmaBuffer::allocate(RX_RING_PAGES)?;
mmio.write32(REG_RBSTART, rx_buffer.pa() as u32);

let tx_buffers = DmaBuffer::allocate(TX_POOL_PAGES)?;
for i in 0..4 {
    let slot_pa = tx_buffers.pa() + (i * TX_BUFFER_SLOT_SIZE) as u64;
    mmio.write32(REG_TSAD0 + i * 4, slot_pa as u32);
}
```

The RTL8139 has 32-bit DMA address registers, so the code casts physical addresses to `u32`. This works only while DMA memory is allocated below 4 GiB. A general implementation would explicitly enforce that condition in the allocator.

`VirtToPhys` supports the related translation operation. It walks the current PML4, PDPT, page directory, and page table. It also recognizes 1 GiB and 2 MiB huge-page mappings and adds the appropriate offset within the huge page.

---

## 9. Why KAOS NIC Drivers Poll Instead of Using Interrupts

An earlier version of this codebase had a full hardware-IRQ bridge for Ring-3 drivers: a kernel module mapping PIC lines to waiting driver tasks, three syscalls (`IrqSubscribe`/`IrqWait`/`IrqAck`), a top-half trampoline, a watchdog that forced an EOI if a driver task sat on an in-service line too long, and matching subscribe/wait calls in both `Rtl8139Device` and `IntelNicDevice`. It was fully wired up and covered by its own kernel test — and neither driver's background event loop (§12) ever actually called into it on the receive path. Both drivers polled `poll_next_packet()` every loop iteration regardless, which is the only thing that ever drove a received frame through the system. The interrupt subscription, once established at startup, sat unused for the rest of the driver's life.

That mismatch — real infrastructure, zero runtime benefit — is exactly the kind of complexity worth removing rather than working around, so the bridge, its syscalls, and the drivers' own subscribe calls were deleted outright. What remains is simple: a NIC driver's background loop (§12) polls `poll_next_packet()` once per iteration, `yield_now()`s, and repeats. There is no notion of "waiting for a packet" anywhere in this codebase's networking path — an idle NIC driver still cycles through its loop and checks hardware every scheduler slice, at whatever rate cooperative scheduling grants it a turn.

This is a deliberate trade-off, not an oversight: polling is trivial to reason about and test, has no risk of a wedged PIC line or a missed wakeup, and this codebase's own goal is to be understandable end to end rather than maximally efficient. The concrete cost is CPU time spent on `poll_next_packet()` calls that find nothing, on every driver, on every scheduler slice, forever — §18 returns to this. A future version wanting genuine interrupt-driven RX would need to reintroduce a mechanism along these lines; nothing about that would need to be novel, since the shape of it (a top-half trampoline, a per-line wait queue, a way to defer EOI until a Ring-3 handler has serviced the device) is well understood prior art in this kind of kernel — it just is not present today.

---

## 10. Initializing the RTL8139 Controller

After being handed off to by its own `_start()`, the driver locates its device via `lib_driver_runtime::find_bound_device()` (§3), chooses a memory BAR via `map_mmio_bar()`, and only then calls into the RTL8139-specific initialization code.

`Rtl8139Device::init()` first disables power saving and requests a software reset:

```rust
mmio.write8(REG_CONFIG1, 0x00);
mmio.write8(REG_CHIPCMD, CMD_RESET);

let mut timeout = 10_000;
while (mmio.read8(REG_CHIPCMD) & CMD_RESET) != 0 && timeout > 0 {
    timeout -= 1;
}

if timeout == 0 {
    return Err(SysError::IoError);
}
```

Reset is asynchronous. Writing starts it, and the hardware clears the bit after completion. The finite loop prevents an unresponsive device from trapping the process forever during initialization.

The driver then reads six consecutive bytes from `REG_MAC0` to obtain the hardware MAC address. It allocates the RX and TX DMA regions described in §8 and writes their physical addresses into the controller.

`CAPR` is the consumer pointer of the receive ring. The RTL8139 expects it to be offset backwards by 16 bytes, so the initial value is `0xFFF0`, which represents logical offset zero minus `0x10` with 16-bit wrapping.

The Receive Configuration Register is programmed as follows:

```rust
let rcr = RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_WRAP;
mmio.write32(REG_RCR, rcr);
```

This accepts all packets, packets matching the physical MAC, multicast, and broadcast. `RCR_WRAP` enables ring-buffer wraparound. Because `AAP` already accepts everything, the software network stack performs an additional destination-MAC check.

The driver does not subscribe to the device's hardware IRQ line at all — §9 explains why — so `REG_IMR` (the interrupt mask register) is left at its post-reset default of "everything masked." Finally, the driver sets the receiver-enable and transmitter-enable bits in `CHIPCMD`.

---

## 11. Transmitting an Ethernet Frame

`Rtl8139Device::transmit(packet)` accepts packets from one to 1792 bytes. The controller has four transmit descriptors. `tx_cur` selects the current descriptor and the matching slot in the DMA pool.

The code first waits for the OWN bit in the Transmit Status Descriptor to indicate that the slot is available. It then copies the packet through the virtual DMA mapping:

```rust
let slot_offset = slot * TX_BUFFER_SLOT_SIZE;
let tx_slice = self._tx_buffers.as_mut_slice();

tx_slice[slot_offset..slot_offset + packet.len()]
    .copy_from_slice(packet);
```

Writing the length to the TSD register hands the buffer to the device and triggers transmission:

```rust
let tx_len = packet.len().max(60);
self.mmio.write32(tsd_reg, tx_len as u32);
self.tx_cur = (self.tx_cur + 1) % 4;
```

Sixty bytes is the minimum Ethernet-frame length without the Frame Check Sequence generated by hardware. Short protocol packets must therefore be padded — `lib_net` itself handles this by construction on every frame it builds, described precisely in `docs/networking.md` §3.1.

The data path is intentionally efficient. Syscalls are needed to allocate and establish mappings, but each packet is written directly to mapped DMA memory and triggered with a direct MMIO write. There is no syscall for every register access. This is the same property that lets `transmit()` be called, unmodified, from deep inside the background loop's per-iteration polling in §12 without becoming the loop's bottleneck.

---

## 12. Receiving a Frame, and the Driver's Permanent Background Loop

The RTL8139 writes incoming packets sequentially into the RX ring. Each packet is preceded by four bytes of device metadata: two status bytes and a two-byte length. The reported length includes the four-byte Ethernet CRC, which the higher network stack does not need.

`poll_next_packet()` first checks `CMD_BUF_EMPTY`. If data is available, it reads status and length at the current `rx_offset`:

```rust
let status = u16::from_le_bytes([
    rx_slice[offset],
    rx_slice[offset + 1],
]);

let length = u16::from_le_bytes([
    rx_slice[offset + 2],
    rx_slice[offset + 3],
]) as usize;
```

The RTL8139 registers and ring metadata use little-endian values here, whereas network protocols use big-endian multi-byte fields later. Driver code frequently crosses several independent binary interfaces, each with its own byte order.

Only packets with the Receive OK bit and a plausible length are accepted. Four CRC bytes are removed from the reported size, and the driver copies the frame into the caller's buffer. The next record begins on a four-byte boundary after the device header and packet. When the offset reaches the end of the 8192-byte ring, it wraps modulo the ring size. Finally, the driver writes `rx_offset - 0x10` to `CAPR`, informing the device how much data software has consumed.

This is the last piece of hardware-specific code either driver ever runs. Once `Rtl8139Device::init()` (or, on the Intel driver, its equivalent) has returned a working `NicDevice` and a freshly constructed `NetworkStack`, `_start()` does nothing else but call [`lib_driver_runtime::run_background_driver()`](../lib_driver_runtime/src/repl.rs) and never returns:

```rust
// drivers/rtl8139/src/main.rs
let stack = NetworkStack::new(mac);
// ...
lib_driver_runtime::run_background_driver(device, stack, "nic:rtl8139")
```

`run_background_driver()` is generic over any `NicDevice`, which is exactly why the Intel driver's `_start()` ends in the identical call with `"nic:intel_nic"` substituted in. Its full body, quoted here because every line of it matters and none of it is hardware-specific, is:

```rust
pub fn run_background_driver<D: lib_net::NicDevice>(
    mut device: D,
    mut stack: lib_net::NetworkStack,
    driver_name: &str,
) -> ! {
    // Step 0: register, then resolve our own packed task id -- there is no
    // separate "get my own tid" syscall, and DrvRegister itself does not
    // return one.
    if let Err(e) = lib_driver::drv::drv_register(driver_name.as_bytes()) {
        lib_kaos::serial_println!("[driver] DrvRegister failed: {:?}", e);
        lib_kaos::process::exit();
    }
    let own_id = match lib_driver::drv::drv_lookup(driver_name.as_bytes()) {
        Ok(id) => id,
        Err(e) => {
            lib_kaos::serial_println!("[driver] DrvLookup of own name failed: {:?}", e);
            lib_kaos::process::exit();
        }
    };

    let mut tx_buf = [0u8; lib_driver::drv::MAX_PACKET_LEN];
    let mut rx_buf = [0u8; lib_driver::drv::MAX_PACKET_LEN];
    loop {
        // Step 1: drain app -> driver TX requests, non-blocking.
        while let Ok(len) = lib_driver::drv::net_recv(own_id, &mut tx_buf, 0) {
            let _ = device.transmit(&tx_buf[..len]);
        }

        // Step 2: poll hardware RX, run them through this driver's own
        // NetworkStack, forward every frame to waiting apps regardless of
        // what the stack did with it.
        while let Some(len) = device.poll_next_packet(&mut rx_buf) {
            let _event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
                let _ = device.transmit(tx_pkt);
            });
            let _ = lib_driver::drv::net_send(own_id, &rx_buf[..len]);
        }

        // Step 3: publish MAC/IP/counters/ARP table for DrvQuery.
        let status = build_status(&stack);
        let _ = lib_driver::drv::publish_status(&status);

        // Step 4: cooperative yield.
        lib_kaos::process::yield_now();
    }
}
```

Reading this loop is really the shortest possible description of what a running NIC driver *is* in KAOS. Step 0 happens exactly once, at startup: the driver announces its own existence under a fixed, hardcoded name — `"nic:rtl8139"` or `"nic:intel_nic"` — through a mechanism, `DrvRegister`, explained in full in the next section. Everything after that is the body of an infinite loop that never sleeps for long and never blocks indefinitely on any single step. Step 1 drains any packets an application has queued for transmission and hands them to the real hardware via the ordinary `transmit()` method from §11. Step 2 polls the real hardware for newly arrived frames via `poll_next_packet()` from earlier in this section, runs each one through this driver's own copy of the `lib_net` protocol stack — which is what actually answers an ARP request or ICMP ping addressed to this machine, entirely on its own, without any application being involved — and then, regardless of what that stack did or didn't do with the frame, forwards a copy of it onward to any application that might be waiting for exactly this traffic. Step 3 publishes a fresh status snapshot every single iteration, so a `DrvQuery` call from an application always sees fairly current counters and ARP entries. Step 4 yields the CPU, because this is cooperative scheduling and a loop this tight would otherwise starve every other task on the machine.

The one detail worth pausing on is Step 0's second half: having just registered itself, the driver immediately turns around and looks its own name up again via `DrvLookup`, purely to learn the packed task id the kernel assigned it. There genuinely is no dedicated "tell me my own task id" syscall, and `DrvRegister` itself returns nothing but success or failure — so the driver ends up as, technically, its own first client of the very lookup mechanism §13 exists to serve external applications.

---

## 13. The Driver Registry: How an Application Finds and Talks to a Driver

This is the layer that did not exist in the very first version of this feature, added specifically to make the shift to permanent background drivers possible. Once a driver never exits and is never waited on by whatever process happened to start it, the process that *wants to use the network* — `net-tools.bin`, or in principle any future application — has no other way to learn which of potentially several already-running tasks is "the network driver" it should talk to. A task id assigned at spawn time is not predictable and not stable across reboots; a name is. The kernel therefore keeps a small, fixed-capacity registry mapping human-readable names, like `"nic:rtl8139"`, to the packed task id currently serving that name, in [`kernel/src/drivers/registry.rs`](../kernel/src/drivers/registry.rs):

```rust
pub struct DriverEntry {
    pub name: [u8; DRIVER_NAME_LEN],
    pub name_len: usize,
    pub tid: usize,
    tx_ring: PacketRing,
    rx_ring: PacketRing,
    status: Option<UserDriverStatus>,
}

static DRIVER_REGISTRY: SpinLock<Vec<DriverEntry>> = SpinLock::new(Vec::new());
```

Two design decisions here are worth calling out explicitly, because they are easy to miss and both are deliberate. First, this registry carries no packet or status data of its own conceptually separate from the naming — it happens to *store* both a `tx_ring`, an `rx_ring`, and a `status` snapshot right there in the same struct, but that is purely a convenience of implementation; the module's own header comment describes itself as "the naming layer" with packet transport and status "layered on top." Second, and much more important for understanding the security model: there is, by design, no shared memory anywhere in this picture. Two Ring 3 processes — an application and a driver — never map the same physical page into both of their address spaces. Every single packet crosses the boundary by being copied into the kernel once, by whichever side is sending, and copied back out of the kernel once, by whichever side is receiving. The kernel is the mailbox, quite literally: `DRIVER_REGISTRY` is a `Vec` sitting in kernel memory, and both `tx_ring` and `rx_ring` are bounded circular buffers of raw bytes, `RING_CAPACITY = 32` slots deep, each slot large enough for `MAX_PACKET_LEN = 1536` bytes — a size chosen to comfortably hold a full Ethernet frame with room to spare.

### 13.1 Registering and Resolving a Name

`DrvRegister` (syscall 36) and `DrvLookup` (syscall 37) are the pair that turn a plain kernel `Vec` into something an application can use. Registering is deliberately the more restricted of the two:

```rust
pub fn syscall_drv_register_impl(name_ptr: u64, name_len: u64) -> SyscallResult<u64> {
    // Step 1: caller must be a driver task.
    scheduler::current_task_caps().ok_or(SyscallError::PermissionDenied)?;
    let (name_buf, name_len) = copy_user_driver_name(name_ptr, name_len)?;
    let tid = scheduler::current_task_id().ok_or(SyscallError::PermissionDenied)?;
    registry::register(&name_buf[..name_len], tid)?;
    Ok(SYSCALL_OK)
}

pub fn syscall_drv_lookup_impl(name_ptr: u64, name_len: u64) -> SyscallResult<u64> {
    let (name_buf, name_len) = copy_user_driver_name(name_ptr, name_len)?;
    registry::lookup(&name_buf[..name_len])
        .map(|tid| tid as u64)
        .ok_or(SyscallError::InvalidArg)
}
```

`DrvRegister`'s very first line is the same `current_task_caps()` check §5 already introduced: it succeeds only for a task whose `caps` pointer is non-null, which is to say, only for a task that was itself spawned via `SpawnDriver`. An ordinary application, no matter how it tries, cannot squat a name in this registry — there is no way for it to obtain a `DriverCaps` block at all except through the delegation path §21 describes, and that path never delegates `SPAWN_DRIVER`-derived driver identity to an app, only the separate `UNLOAD_DRIVER`/`LIST_DRIVERS` bits `DRIVERS.BIN` needs. `DrvLookup`, by contrast, has no such check at all — resolving a name to a task id is not itself a privileged operation, and any process, including one with zero capabilities, may call it. This asymmetry is exactly why `net-tools.bin` can talk to a driver despite holding no `DriverCaps` of its own: it never needs to register anything, only to look an existing registration up.

`registry::register()` fails (always with the same `InvalidArg`, since there is no dedicated "already exists" or "full" error code in this codebase) if the name is longer than `DRIVER_NAME_LEN = 32` bytes, if the registry already holds its maximum of `MAX_DRIVERS = 16` entries, or if the exact same name is already registered by anyone, including the calling task itself re-registering. That last case is deliberate: `DrvRegister` is meant to be called exactly once, in a driver's own startup path, not treated as an idempotent "update my registration" call.

### 13.2 Sending and Receiving Packets: the Role-Based Direction Rule

The single most important, and least obvious, design decision in this whole layer is that there are only two packet-transport syscalls, `NetSend` (38) and `NetRecv` (39), not four. A naive design would have separate "send to driver" / "receive from driver" calls for an application and separate "send to app" / "receive from app" calls for the driver. KAOS instead gives both roles the same two syscalls, and lets the *direction* of the copy be decided by comparing the calling task's own identity to the `driver_id` argument:

```rust
// kernel/src/syscall/dispatch/driver.rs, doc comment on NetSend:
// - Caller is an ordinary app (its own tid != driver_id): pushes into
//   driver_id's TX ring (App -> Driver).
// - Caller *is* the driver itself (its own tid == driver_id): pushes
//   into its own RX ring (Driver -> App).
```

The implementation is a single boolean test, computed once and threaded through to the registry:

```rust
let caller_is_driver = scheduler::current_task_id() == Some(driver_id as usize);
registry::push_packet(driver_id as usize, caller_is_driver, &packet_buf[..packet_len])?;
```

and, inside the registry itself:

```rust
pub fn push_packet(driver_id: usize, caller_is_driver: bool, data: &[u8]) -> Result<(), SyscallError> {
    let outcome = with_entry_mut(driver_id, |entry| {
        let ring = if caller_is_driver { &mut entry.rx_ring } else { &mut entry.tx_ring };
        ring.push(data)
    });
    outcome.unwrap_or(Err(SyscallError::InvalidArg))
}
```

Once this rule clicks, both call sites in this document read naturally. When `net-tools.bin` calls `NetSend` with the driver's id, its own tid is obviously not equal to the driver's, so the frame lands in the driver's TX ring — literally "transmit," from the driver's point of view, which is exactly where §12's Step 1 drains it from. When the driver itself, inside its own background loop, calls `NetSend` on its own id (`lib_driver::drv::net_send(own_id, &rx_buf[..len])` in §12's Step 2), its tid trivially equals `driver_id`, so the very same syscall instead lands the frame in the driver's RX ring — "receive," again from the driver's point of view, which is where an application's own `NetRecv` call goes looking for it. One syscall, one small equality check, and both halves of a bidirectional channel fall out of it without ever needing a notion of "the other side's identity" more complicated than a task id comparison.

`NetRecv` mirrors this exactly, popping from the *opposite* ring `NetSend` would have pushed into for the same `caller_is_driver` value, and adds one more piece of behavior worth knowing precisely: `timeout_ms == 0` means "poll the ring exactly once, and return `SysError::Timeout` immediately if it was empty" — not "wait forever." This is not an oversight; it exists because the driver's own background loop (§12, Step 1) calls `net_recv(own_id, &mut tx_buf, 0)` in a `while let Ok(len) = ...` drain loop specifically because it must never block there even for a moment — a driver that blocked waiting for an application to send it something would stop servicing hardware RX and status publishing for as long as no application had anything to say. A non-zero `timeout_ms` does support a genuine bounded wait, but even that is implemented without ever calling the scheduler's ordinary `block_task()` — a blocked task only resumes when something else unblocks it, which would defeat a timeout entirely if no producer ever shows up. Instead, the kernel busy-polls cooperatively, calling `scheduler::yield_now()` in a loop and checking the ring again after every yield, until either a packet appears or a `RDTSC`-based deadline passes:

```rust
let ticks_per_ms = time::tsc_ticks_per_us().saturating_mul(1000);
let deadline = time::rdtsc().saturating_add(ticks_per_ms.saturating_mul(timeout_ms));

loop {
    scheduler::yield_now();
    match registry::try_pop_packet(driver_id as usize, caller_is_driver, &mut kernel_buf[..copy_cap]) {
        Ok(Some(n)) => { /* copy out and return Ok(n) */ }
        Ok(None) => {}
        Err(e) => return Err(e), // the driver exited mid-wait
    }
    if time::rdtsc() >= deadline {
        return Err(SyscallError::Timeout);
    }
}
```

The underlying `PacketRing` itself never blocks either, in either direction: `push()` simply fails with `InvalidArg` if the ring already holds `RING_CAPACITY` unread packets, and the caller — whichever side that happens to be — is expected to treat a full ring as backpressure, not as something worth waiting out. §18 returns to what this means in practice.

### 13.3 Status Publishing: `DrvPublishStatus` and `DrvQuery`

Raw packets alone would let an application send and receive Ethernet frames, but §14's walkthrough of `net-tools.bin` needs more than that before it can even construct its first frame: it needs to know the driver's MAC address, its configured IP address, subnet mask, gateway, and DNS server, and — for the `arp` command — the driver's currently resolved ARP table. `DrvPublishStatus` (40) and `DrvQuery` (41) exist purely to carry this snapshot across the process boundary, following the same "the kernel is the mailbox, nothing is ever shared" rule as the packet rings.

The driver publishes a complete `UserDriverStatus` struct once per loop iteration (§12, Step 3), and any application can request the latest snapshot at any time:

```rust
#[repr(C)]
pub struct UserDriverStatus {
    pub mac: [u8; 6],
    pub _padding0: [u8; 2],
    pub ip: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub link_up: u8,
    pub _padding1: [u8; 3],
    pub arp_entry_count: u32,
    pub arp_entries: [UserArpEntry; MAX_ARP_ENTRIES],
}
```

The explicit `_padding` fields exist for exactly the reason `UserDriverInfo`'s padding does elsewhere in this codebase: this struct is copied wholesale, byte for byte, across the syscall boundary with `read_unaligned`/a raw pointer write, so its layout must be identical and predictable on both sides regardless of what the Rust compiler might otherwise choose to do with alignment. `build_status()`, in [`lib_driver_runtime/src/repl.rs`](../lib_driver_runtime/src/repl.rs), constructs one of these from the driver's live `NetworkStack` every iteration, truncating the ARP table to `MAX_ARP_ENTRIES` (16) if it happens to hold more resolved hosts than that, and always reporting `link_up: 1` — an honest limitation the code's own comment acknowledges: `NicDevice` has no link-state query at all today, so "the interface reached its background loop, meaning hardware initialization already succeeded" is the best proxy for "link up" currently available.

`DrvPublishStatus` requires the caller to already be registered — it looks itself up by the calling task's own tid, and fails if no `DriverEntry` exists for it, which in practice can only happen if a task calls it without ever having called `DrvRegister` first. `DrvQuery`, symmetrically with `DrvLookup`, is callable by anyone and simply returns `InvalidArg` if the named driver id has never published anything yet.

### 13.4 The Ring-3 Wrapper: `lib_driver::drv` and `NicClient`

Every syscall described above has a thin, typed wrapper in [`lib_driver/src/drv.rs`](../lib_driver/src/drv.rs) — `drv_register()`, `drv_lookup()`, `net_send()`, `net_recv()`, `publish_status()`, `query_status()`, plus `unload_driver()` and `list_drivers()` for the two syscalls §20 covers. None of these do anything beyond validating an argument's length locally (rejecting an empty or oversized name before ever trapping into the kernel) and translating the raw `Result<u64, SysError>` the syscall returns into whichever richer type makes sense — `query_status()`, for instance, allocates an uninitialized `UserDriverStatus` on the stack, passes its address to the kernel, and only calls `assume_init()` after confirming the syscall actually succeeded.

For an application that only ever wants to be a *client* of a running driver — never a driver itself — [`lib_driver::client::NicClient`](../lib_driver/src/client.rs) collapses the four calls it actually needs into a small struct:

```rust
pub struct NicClient {
    driver_id: u64,
}

impl NicClient {
    pub fn open(name: &str) -> Result<Self, SysError> {
        let driver_id = drv::drv_lookup(name.as_bytes())?;
        Ok(Self { driver_id })
    }
    pub fn send(&self, frame: &[u8]) -> Result<(), SysError> {
        drv::net_send(self.driver_id, frame)
    }
    pub fn recv(&self, buf: &mut [u8], timeout_ms: u64) -> Result<usize, SysError> {
        drv::net_recv(self.driver_id, buf, timeout_ms)
    }
    pub fn query_status(&self) -> Result<UserDriverStatus, SysError> {
        drv::query_status(self.driver_id)
    }
}
```

`NicClient` is deliberately a thin veneer with no state of its own beyond the resolved `driver_id` — it holds no packets, no cache, nothing that would need cleanup. This is the type `net-tools.bin` actually uses, and the next section follows it through a complete session.

---

## 14. `net-tools.bin`: A Complete Application Walkthrough

Everything in §13 exists to make the following program possible: an ordinary Ring 3 application, holding no `DriverCaps`, mapping no MMIO, allocating no DMA memory, that nonetheless sends and receives real Ethernet frames and gets a real `ping` reply from a real (or emulated) remote host. [`user_programs/net_tools`](../user_programs/net_tools/src/main.rs) — shipped on the disk image as `NETTOOLS.BIN` — is that application, and its own header comment states its purpose plainly: it is "a standalone Ring-3 network utility that talks to whichever NIC driver is currently loaded, instead of any network functionality being baked into the driver binaries themselves." Everything network-specific used to live inside the driver's own, now-removed foreground CLI; today it lives here, completely separated from anything that touches hardware.

### 14.1 Finding a Driver Without Knowing Which One Is Running

`net-tools.bin` does not assume any particular driver is loaded. The machine might have an RTL8139 or an Intel NIC, or, at the moment the user runs `net-tools`, no driver at all — perhaps nobody ran `load` in `DRIVERS.BIN` yet. Its `_start()` therefore probes a short, ordered list of the only names any driver in this codebase ever registers under:

```rust
const KNOWN_DRIVER_NAMES: &[&str] = &["nic:rtl8139", "nic:intel_nic"];

let Some(driver_name) =
    probe_driver_name(KNOWN_DRIVER_NAMES, |name| NicClient::open(name).is_ok())
else {
    println!("No NIC driver loaded. Run 'load <name>.drv' in drivers.bin first.");
    process::exit();
};
```

`probe_driver_name()` is nothing more than trying `NicClient::open()` — which is to say, a `DrvLookup` — against each name in turn and stopping at the first one that resolves. If neither name is currently registered, the program prints a helpful pointer back to `DRIVERS.BIN` and exits immediately; there is nothing more it can do without a driver to talk to.

### 14.2 Seeding a Local, Independent `NetworkStack`

Having found a name that resolves, `net-tools.bin` opens a real `NicClient` and immediately calls `query_status()` — a `DrvQuery` under the hood — to learn the driver's MAC address and network configuration:

```rust
let status = client.query_status()?;
let mac = MacAddress::new(status.mac);
let mut stack = NetworkStack::new(mac);
stack.config.ip = Ipv4Address::new(status.ip[0], status.ip[1], status.ip[2], status.ip[3]);
stack.config.subnet_mask = Ipv4Address::new(status.subnet_mask[0], /* ... */);
stack.config.gateway = Ipv4Address::new(status.gateway[0], /* ... */);
stack.config.dns = Ipv4Address::new(status.dns[0], /* ... */);
```

This is worth pausing on, because it is easy to assume `net-tools.bin` somehow shares the driver's own `NetworkStack`, and it emphatically does not. It constructs a **second, completely independent** `NetworkStack` value, seeded once at startup from a snapshot of the driver's configuration, and never touched again after that except by `net-tools.bin`'s own subsequent traffic. In particular, its `ArpTable` starts out empty and evolves completely independently of the driver's own table from this point on: the driver's table keeps learning passively from every frame that crosses the wire, addressed to it or not (`docs/networking.md` §4.3 explains why), while `net-tools.bin`'s own table only ever grows from replies to ARP requests `net-tools.bin` itself issued. This is precisely why its `arp` command does not print its own `NetworkStack::arp_table` at all — it calls `query_status()` fresh, every time, and prints the driver's table instead, because that is the more complete and more useful one for a user actually inspecting the network.

### 14.3 The REPL and Its Three Real Commands

From here `net-tools.bin` is an ordinary line-based REPL, structurally identical to the shell's own loop and to `DRIVERS.BIN`'s: print a `net-tools>` prompt, read a line via `console::readline()`, split it into a command word and the remainder, and dispatch. `help`, `exit`, and `quit` do the obvious thing. `arp` and `ifconfig` both simply call `client.query_status()` again and format the result — always a fresh `DrvQuery`, never a cached value. `ping <ip>` is the one command that actually exercises everything §13 built, and following it end to end is the best way to see the whole system work together.

### 14.4 `ping`, Traced Step by Step

`execute_ping()` begins with a routing decision that has nothing to do with any driver yet: it checks whether the target address shares a subnet with `net-tools.bin`'s own configured IP (octet-by-octet, masked by `stack.config.subnet_mask`). If it does not, and no default gateway is configured, the command fails immediately with "Destination Host Unreachable" — there genuinely is no routing table beyond this single gateway-or-direct choice, matching `docs/networking.md` §9.3's description of the identical logic.

Assuming the target is reachable, the next hop's MAC address must be known before any IPv4 packet can be framed at all, since Ethernet delivers by MAC, not by IP. If the address is not already in `net-tools.bin`'s own (freshly-empty, per §14.2) ARP table, the code constructs a broadcast ARP request using `lib_net`'s pure, hardware-agnostic `build_arp_request()`, and sends it with exactly one line that actually crosses the process boundary:

```rust
let _ = client.send(&arp_buf[..arp_len]);
```

This is a `NetSend` call with `net-tools.bin`'s own tid *not* equal to the driver's tid, so — per §13.2's role-based rule — the frame lands in the driver's TX ring. Nothing happens instantaneously: the frame sits in that ring until the driver's own background loop (§12) next reaches Step 1, calls its non-blocking `net_recv(own_id, ...)` drain, finds the queued ARP request, and calls `device.transmit()` on it — the very first real MMIO write and real DMA transfer this whole `ping` invocation triggers. From this point, `net-tools.bin` polls for a reply for up to twenty seconds, retransmitting every two seconds, draining `client.recv(&mut rx_buf, 0)` — non-blocking, per §13.2 — in a tight loop and feeding whatever arrives through its own `NetworkStack::handle_rx_packet()`, which is what actually populates its ARP table once a reply shows up.

The reply itself takes a path worth tracing precisely, because it demonstrates something easy to miss: the driver's background loop answers ARP requests addressed to *itself* on its own, with no application involved at all, but a reply addressed to `net-tools.bin`'s own request is a different case. When the remote host's ARP reply arrives over the wire, the driver's `poll_next_packet()` (§12, Step 2) picks it up, the driver's own `NetworkStack::handle_rx_packet()` recognizes it as a reply rather than a request (`docs/networking.md` §4.4 covers this dispatch precisely) and does nothing with it beyond updating its own, separate ARP cache and returning an event the driver loop simply discards — but the very next line in that same loop, unconditionally, regardless of what the stack did or didn't do with the frame, forwards a copy of the raw frame onward:

```rust
let _ = lib_driver::drv::net_send(own_id, &rx_buf[..len]);
```

This is the driver calling `NetSend` on its *own* id, so — again by the role-based rule — it lands in the driver's RX ring, precisely where `net-tools.bin`'s next `client.recv()` poll picks it up. Only at that point does `net-tools.bin`'s own second `NetworkStack::handle_rx_packet()` run against the same bytes, this time actually populating its own ARP table with the resolved MAC address, letting the polling loop above break out successfully.

With the destination MAC now known, four ICMP Echo Requests follow, one per sequence number one through four, each built by `lib_net`'s `build_ping()` and sent the same way the ARP request was — `client.send()`, landing in the driver's TX ring, drained and transmitted by the driver's background loop. Each one is followed by up to two seconds of the same `client.recv()`-then-`handle_rx_packet()` polling pattern, this time watching specifically for a `NetworkEvent::IcmpEchoReply` whose source IP, identifier (`net-tools.bin` always uses the fixed value `0x1337`), and sequence number all match what was just sent — any other event arriving on the same shared channel, an unrelated ARP packet, or a ping reply meant for a different identifier entirely, is simply ignored, because the driver's RX ring is not filtered per-application and can and does deliver traffic that has nothing to do with this particular `ping` invocation. Round-trip time is measured with the CPU's own timestamp counter (`RDTSC`) around the `client.send()`/matching-event pair, converted to milliseconds by assuming a fixed 2 GHz TSC frequency — accurate only under the QEMU configuration this codebase targets, not a general-purpose time source.

Tracing one single successful `ping 192.168.1.1`, then, crosses the process/hardware boundary like this: `net-tools.bin` (frame construction in `lib_net`) → `NetSend` syscall → driver's TX ring → driver's background loop drains it → `device.transmit()` (real MMIO/DMA) → wire → remote host → wire → driver's `device.poll_next_packet()` (real MMIO/DMA) → driver's own `NetworkStack::handle_rx_packet()` (recognizes a reply, the driver loop discards the returned event) → unconditional `NetSend` back onto the driver's own RX ring → `net-tools.bin`'s `client.recv()` → `net-tools.bin`'s own, second `NetworkStack::handle_rx_packet()`, whose returned `NetworkEvent::IcmpEchoReply` is what finally prints the `64 bytes from ... time=... ms` line the user actually sees. Four syscalls, two independent copies of the exact same protocol-stack code, and not one byte of memory ever shared between the two processes.

---

## 15. Resource Lifetime and Shutdown, Now That Drivers Never Exit

Rust's RAII model remains useful for the resources a driver itself owns. `Mmio` unmaps its region when dropped, and `DmaBuffer` unmaps and frees its frames (§7, §8). `Rtl8139Device` directly owns all of them, and the `NicDevice` trait's `shutdown()` method exists specifically to disable the receiver and mask interrupts before a controlled teardown:

```rust
pub fn shutdown(&mut self) {
    self.mmio.write8(REG_CHIPCMD, 0x00);
    self.mmio.write16(REG_IMR, 0x0000);
}
```

The honest thing to say about this method today is that nothing in the normal running path ever calls it. This is a direct, structural consequence of §12's architecture: `run_background_driver()` is declared to return `!` and, true to that promise, never does under normal operation. There is no `exit`/`quit` command inside a driver process at all anymore — unlike the shell or `net-tools.bin`, a NIC driver simply has no code path that leads back out of its own infinite loop. `process::exit()` itself has return type `!` and performs no Rust stack unwinding when called, so even if some future code path did call it, local variables — including a `device: Rtl8139Device` sitting on `run_background_driver()`'s own stack frame — would not be dropped through the ordinary route in any case.

The only way a driver task ever stops today is `DrvUnload` (syscall 42), and it is deliberately, explicitly documented as a hard kill rather than a cooperative shutdown request:

```rust
pub fn syscall_drv_unload_impl(name_ptr: u64, name_len: u64) -> SyscallResult<u64> {
    // ... authorization + name resolution ...
    let tid = registry::lookup(&name_buf[..name_len]).ok_or(SyscallError::InvalidArg)?;

    // Step 4: hard-kill the task. `terminate_task` -> `remove_task` already
    // releases this driver's registry entry, MMIO/DMA allocations, and PCI
    // device reservation.
    if !scheduler::terminate_task(tid) {
        return Err(SyscallError::InvalidArg);
    }
    Ok(SYSCALL_OK)
}
```

`terminate_task()` reaches the scheduler's single common cleanup path, `remove_task()`, the same choke point reached whether a task exits normally, crashes, or is killed like this. That one function is what actually calls `driver_db::release_task()` (releasing the PCI device reservation so a future `load` can reclaim it) and `registry::release_task()` (removing the now-dead entry from the very registry §13 describes, so a stale `DrvLookup` can never resolve to a task id that no longer exists) — plus the ordinary VMM teardown that reclaims every PMM-owned frame the task's address space still referenced, including its DMA buffers. What none of this does is run a single instruction of the driver's own code: `shutdown()` is never invoked, so the NIC itself is never told to stop, its receiver and interrupt mask are never disabled, and if it was mid-transmission when the kill happened, the hardware's own view of its DMA buffers becomes stale the instant those physical frames are returned to the PMM's free list and handed to something else.

§18 discusses the resulting risk in the context of KAOS's other DMA-safety limitations; the short version repeated here for completeness is that this is a known, accepted gap rather than an oversight, and building a cooperative shutdown protocol — a dedicated syscall a driver polls for, mirrored by `unload` waiting for an acknowledgement before killing the task — remains future work.

The ownership picture, updated for the current architecture, looks like this:

```text
TaskEntry
  `-- DriverCaps
        |-- capability bits (MMIO only)
        `-- MMIO grants

Rtl8139Device (never dropped in the normal, no-exit run path)
  |-- Mmio         --Drop, if ever reached--> UnmapPhysical
  |-- RX DmaBuffer --Drop, if ever reached--> FreeDma
  `-- TX DmaBuffer --Drop, if ever reached--> FreeDma

DrvUnload -> scheduler::terminate_task -> remove_task
  |-- driver_db::release_task    (PCI device reservation)
  |-- registry::release_task     (name -> tid entry, packet rings, status)
  `-- VMM teardown                (every PMM-owned frame, including DMA)
```

MMIO and DMA still have different ownership semantics underneath all of this. For MMIO, the device owns the physical address and the process owns only a virtual mapping. For DMA, the physical RAM frames were allocated for the process and must eventually return to the PMM — which they do, correctly, on this hard-kill path, just without the device ever being told to stop touching them first.

---

## 16. Build Integration, ELF Layout, and QEMU

`lib_driver`, `lib_driver_runtime`, `lib_net`, and the two driver crates (`drivers/rtl8139`, `drivers/intel_nic`) are Cargo workspace members, built for `x86_64-unknown-none` without the standard library. They use `core`, the existing allocator through `alloc`, `lib_kaos`, and each other. `user_programs/drivers` and `user_programs/net_tools` are ordinary user-program workspace members built the same way.

Each driver's linker script selects `_start` as the entry point and places code at its own reserved virtual address, with two page-aligned `PT_LOAD` segments — one readable and executable for code and read-only data, one readable and writable for data and BSS. Page alignment between the segments is necessary because the kernel enforces ELF permissions at page granularity; if executable code and writable data shared one page, the loader could not represent the intended permissions cleanly.

The build helpers copy every relevant binary onto the disk image under its own FAT32 8.3 name:

```text
drivers/rtl8139/rtl8139.bin        ->  RTL8139.DRV
drivers/intel_nic/intel_nic.bin    ->  INTLNIC.DRV
user_programs/net_tools/net_tools.bin  ->  NETTOOLS.BIN
user_programs/drivers/drivers.bin      ->  DRIVERS.BIN
```

The `.DRV` extension for the two hardware drivers is purely a naming convention `DRIVERS.BIN`'s `load` and the kernel's own `driver_db::DRIVER_DB` (§4) agree on; it carries no special meaning to the VFS or the ELF loader, which treat it exactly like any other file.

QEMU is given an emulated RTL8139 device (and, depending on configuration, an emulated Intel NIC) so both driver binaries have real, matching hardware to attach to under emulation. On macOS, the scripts use `vmnet-bridged` with `en0`. On Linux, they expect a preconfigured TAP interface named `tap0`. The guest is therefore attached to a bridged Layer 2 network rather than only an isolated virtual network — which is what makes it possible for `net-tools.bin`'s `ping` to reach an actual host outside the emulated machine at all.

---

## 17. What the Tests Actually Verify

The test suite mirrors the architecture's layers closely. [`capabilities_test.rs`](../kernel/tests/capabilities_test.rs) checks bit operations, the capability-free initial state of ordinary tasks, and attachment and cleanup of a `DriverCaps` block.

[`driver_mmio_test.rs`](../kernel/tests/driver_mmio_test.rs) simulates tasks with and without MMIO authority, verifying rejection of missing capabilities, incorrect physical ranges, zero lengths, and overflowing addresses, plus the successful bump-pointer-advance-and-unmap case. [`driver_spawn_test.rs`](../kernel/tests/driver_spawn_test.rs) checks missing `SPAWN_DRIVER` authority, invalid user pointers, the exact ABI layout of `UserDriverGrants`, that `SPAWN_DRIVER` itself is masked out of a spawned driver's capabilities, that the driver database resolves registered binaries case-insensitively while rejecting unregistered ones, and — both as a pure function and through the `DrvProbe` syscall itself — that `device_present()` tells "unknown driver" and "known driver, no matching device" apart. [`driver_rtl8139_test.rs`](../kernel/tests/driver_rtl8139_test.rs) combines MMIO and DMA in a simulated RTL8139 task, importing the real network modules through `#[path]` so the same parsers and serializers run in ordinary host unit tests and inside the QEMU kernel test environment alike.

The registry and IPC layer §13 describes has its own dedicated coverage: [`driver_registry_test.rs`](../kernel/tests/driver_registry_test.rs) exercises `DrvRegister`/`DrvLookup` directly — duplicate names, a full registry, the capability gate on registration, and the asymmetric "lookup needs no capability" rule. [`net_ring_test.rs`](../kernel/tests/net_ring_test.rs) drives `NetSend`/`NetRecv` against a single simulated task context, including the role-based direction rule and ring backpressure once `RING_CAPACITY` is exceeded. [`net_ring_wakeup_test.rs`](../kernel/tests/net_ring_wakeup_test.rs) goes further and proves the bounded-wait path in §13.2 genuinely works under real preemptive scheduling — a producer task and a blocked-with-timeout `NetRecv` caller both run as real, separately scheduled tasks, since a cooperative `yield_now()`-based poll loop cannot be exercised meaningfully any other way. [`driver_status_test.rs`](../kernel/tests/driver_status_test.rs) covers `DrvPublishStatus`/`DrvQuery`, including the `arp_entry_count` bounds check that protects `DrvQuery` consumers from an out-of-range read. [`driver_unload_test.rs`](../kernel/tests/driver_unload_test.rs) and [`driver_background_loop_test.rs`](../kernel/tests/driver_background_loop_test.rs) round this out: the former exercises `DrvUnload`'s authorization and its full cleanup fan-out through `remove_task`, and the latter — whose own header comment explains why it exists in this particular shape — replicates `run_background_driver()`'s exact steps directly against real kernel syscalls, since the function itself cannot easily be called from a test harness (it never returns, by design).

Both `net-tools.bin`'s and `DRIVERS.BIN`'s pure, I/O-free logic is unit-tested on an ordinary host without touching a syscall at all, following the same convention throughout this codebase: `parse_command()`/`parse_command_line()`, `format_arp_table()`/`format_ifconfig()`, `parse_ping_target()`, and `probe_driver_name()` are all pure functions with their own `#[cfg(test)]` module, entirely separate from the syscall-touching code around them that only the kernel-side integration tests above can meaningfully exercise. `DRIVERS.BIN`'s `load` no longer has a pure, host-testable half of its own at all — the PCI-ID matching it used to do client-side now happens entirely inside the kernel (`driver_db::device_present()`, exposed via `DrvProbe`), covered by the kernel-side tests §17 references instead. [`test_all.sh`](../test_all.sh) runs user-space protocol tests, kernel tests under QEMU, `cargo fmt --check`, and Clippy, then produces a unified summary.

---

## 18. Security and Implementation Limits

This architecture isolates driver code from the kernel much better than a Ring 0 driver, and the move to permanent background services with a kernel-mediated IPC layer closes off the specific race that made the old foreground model fragile — but it is not yet equivalent to a production driver framework, and several of its rough edges are worth naming precisely rather than glossing over.

The most important hardware limitation remains DMA without an IOMMU. The kernel gives the driver physical addresses, and a bus-master device can initiate physical memory transactions. Capabilities prevent the process from creating arbitrary CPU mappings, but they do not configure an IOMMU that limits the frames reachable by the device. A wrongly programmed or malicious DMA device could therefore access memory outside its intended buffers — and, as §15 describes, a driver killed via `DrvUnload` while a transfer was in flight is a second, concrete way this same class of risk can materialize, since the hardware is never told to stop before its buffers are reclaimed and reused.

DMA bookkeeping is also minimal. `FreeDma` does not consult a per-task list of DMA allocations; it translates the supplied virtual pages, removes their mappings, and releases the resulting PFNs. A robust implementation should record the owner, base address, and length of every allocation and accept only exact matching frees.

The driver registry and packet-ring layer has its own, newer set of limitations, all a direct consequence of favoring simplicity over exhaustive robustness in this educational codebase. A `PacketRing` never blocks a producer — a full ring (more than `RING_CAPACITY = 32` unread packets queued in one direction) simply rejects the next `push()` with `InvalidArg`, silently from the sender's perspective if that error is ignored, which both `net-tools.bin`'s ARP/ping sends and the driver's own forwarding calls in fact do (`let _ = ...`). A sustained mismatch between how fast one side produces packets and how fast the other drains them therefore drops traffic rather than exerting any real backpressure a caller could react to. There is also no protection against name-squatting beyond first-come-first-served registration order: any task holding a non-null `DriverCaps` block — which today only ever means a task spawned via `SpawnDriver` — can register any name at all, including one that happens to collide with a name a future, better-behaved driver might want; `driver_db`'s own binary-name-to-PCI-ID table is what keeps this from being exploitable in practice today, but the registry itself enforces no relationship between a name and the identity of whoever is allowed to claim it. Two independent `NetworkStack` instances — the driver's own and every application's separately seeded copy — genuinely diverge over the life of a session, as §14.2 describes for `net-tools.bin`'s ARP table specifically; this is a deliberate simplicity trade-off (no shared-state synchronization protocol exists or is planned), not a bug, but it does mean an application's view of "what the network looks like" is only ever as fresh as its last `DrvQuery`.

The MMIO bump allocator never reuses virtual holes. A long-lived process that repeatedly maps and unmaps regions still advances toward the stack guard. This simple design is adequate for a NIC driver's mostly one-time BAR and DMA setup at startup, which is the only time either concrete driver in this codebase actually calls `MapPhysical`/`AllocDma` at all.

As §9 describes, neither NIC driver uses hardware interrupts at all — both poll `poll_next_packet()` every scheduler slice regardless of whether a frame has actually arrived. This is a deliberate simplicity trade-off, but it does mean an idle NIC driver still burns a full scheduling turn every cycle checking hardware that has nothing new to report.

In the transmit path, the descriptor wait loop expires after 10,000 iterations without returning an error; the code still writes the slot and starts transmission regardless. It also reports at least 60 bytes for short Ethernet frames without explicitly zeroing the additional bytes before every transmission — data left over from a previous use of that slot could in principle be emitted as Ethernet padding, though `lib_net` itself always hands the driver a frame it has already zero-padded correctly (`docs/networking.md` §3.1), so this is currently a latent risk rather than an observed one.

The protocol stack the driver hands frames to has its own, separately documented limitations — no DHCP, no TCP/UDP, no IP fragmentation, ARP entries with no expiration timer, and so on; see `docs/networking.md` §11.

Finally, QEMU networking depends on host configuration. `en0` is not the active interface on every Mac, and Linux requires `tap0` to be created with appropriate permissions and attached to a bridge before the scripts run.

---

## 19. The Entire Path as One Continuous Story

During boot, the kernel scans the PCI bus. It reads vendor IDs, device IDs, BARs, and IRQ lines. QEMU provides emulated RTL8139 and/or Intel NIC hardware for the guest to find there.

At some later point, the user runs `drivers.bin` from the shell — an `Exec` call that, uniquely for this one binary name, delegates `SPAWN_DRIVER`, `UNLOAD_DRIVER`, and `LIST_DRIVERS` to it (§21). Inside its own tiny REPL, the user types `load rtl8139.drv`. `DRIVERS.BIN` asks the kernel via `DrvProbe` (§4) whether a matching device is actually present on the live PCI bus — without keeping any PCI-ID table of its own — and calls `SpawnDriver` — which validates the caller's capability, derives the actual MMIO grant from the kernel's own view of that device (never trusting anything the caller supplied), spawns the new task in a blocked state, attaches its grant, confirms the PCI device reservation, and only then unblocks it. `DRIVERS.BIN` prints the new task's id and its own prompt returns immediately — there is no waiting, and from this point `DRIVERS.BIN` and the driver it just started have nothing more to do with each other.

The new process discovers its device again on its own, maps the granted BAR into its MMIO window, initializes the controller, allocates and wires up DMA rings, and constructs its own `NetworkStack`. It then calls `run_background_driver()` and, from that call onward, never executes another line of code outside that function's own infinite loop: registering itself under a fixed name in the kernel's driver registry, then forever draining queued outbound packets, polling real hardware for inbound ones, running each through its own protocol stack, forwarding every inbound frame onward to whoever might be listening, publishing a fresh status snapshot, and yielding.

Independently, and at any later time, the user runs `net-tools.bin`. It has no hardware access whatsoever. It resolves the driver's name through the same registry the driver registered itself in, reads a status snapshot to learn the driver's MAC and IP configuration, and seeds its own, entirely separate copy of the same protocol stack. When the user types `ping 192.168.1.1`, that second `NetworkStack` constructs a complete Ethernet/ARP or Ethernet/IPv4/ICMP frame exactly the way the driver's own stack would, and hands it to the kernel via `NetSend` — which, because the calling task is not the driver, lands it in the driver's TX ring. The driver's background loop, running as its own independently scheduled task and utterly unaware that anything just happened until its next iteration reaches Step 1, drains that ring and transmits the frame over real MMIO and DMA. A reply arrives over the wire; the driver's `poll_next_packet()` picks it up, the driver's own stack looks at it and does nothing further with it beyond updating its own cache, and the very same loop iteration unconditionally forwards a copy back through `NetSend` — this time as the driver, landing it in its own RX ring — where `net-tools.bin`'s own polling `NetRecv` call picks it up, runs it through its own stack a second time, and finally recognizes the matching `IcmpEchoReply` event that lets it print a result to the user.

If the user later runs `unload rtl8139` in `DRIVERS.BIN`, the kernel resolves the name in the registry one last time and hard-kills the driver task outright — releasing its PCI device reservation, its registry entry, and every PMM-owned frame its address space held, but never running a single instruction of the driver's own shutdown code, since there is no cooperative protocol asking it to run any.

The system therefore demonstrates a complete vertical slice through an operating system: PCI configuration, page tables, process permissions, syscalls, DMA ring buffers, kernel-mediated inter-process packet transport, and network protocols — three genuinely independent, mutually distrusting Ring 3 processes (a driver-lifecycle manager, a permanent driver service, and an on-demand client application) cooperating entirely through capability-checked syscalls and a small kernel-owned registry, never once sharing a page of memory with one another.

---

## 20. The `DRIVERS.BIN` Management Application

Loading a driver used to be a single, blocking shell command with no counterpart for unloading one or seeing what was currently running. `DRIVERS.BIN` (built from [`user_programs/drivers`](../user_programs/drivers)) replaces that with a small, standalone Ring 3 REPL dedicated to driver lifecycle management, structurally identical to the shell's own read-eval-print loop: a prompt, a line read via `console::readline()`, and a match over the first whitespace-separated word.

It understands four commands. `help` prints the command list. `list` calls `DrvList` once, into a stack buffer sized to the registry's fixed `MAX_DRIVERS` capacity — reading from the very same registry §13 describes, so this command shows exactly the names any application would be able to resolve via `DrvLookup` at that same instant — and prints every currently registered driver's name and packed task id, or a note that none are loaded. `load <name.drv>` is §4's `load_driver()`. `unload <name>` calls `DrvUnload`, §15's hard-kill.

An illustrative session, after typing `drivers.bin` at the shell prompt to launch it:

```
> drivers.bin
========================================
    KAOS - Driver Management (DRIVERS.BIN)
========================================
Type 'help' to see the list of commands.

drivers> list
No drivers loaded.
drivers> load rtl8139.drv
[drivers] Driver 'RTL8139.DRV' started as TID 2
drivers> list
Loaded drivers:
  nic:rtl8139          tid=2
drivers> exit
```

Note that the prompt returned to the shell after `exit` while the driver, TID 2, keeps running completely independently in the background — a second shell session, or `net-tools.bin` run afterward, would still find `"nic:rtl8139"` registered and fully functional. Unloading it later, from a fresh `drivers.bin` session, looks like this:

```
drivers> list
Loaded drivers:
  nic:rtl8139          tid=2
drivers> unload nic:rtl8139
[drivers] Driver 'nic:rtl8139' unloaded.
drivers> list
No drivers loaded.
```

`load`/`unload`'s command-line parsing (present vs. missing argument, trailing words ignored, case-sensitive dispatch) is factored into a pure `parse_command()` function and unit-tested on the host without touching a syscall, the same pattern this document's §17 describes for `net-tools.bin`'s pure functions. The syscall-touching behavior of `list`/`load`/`unload` themselves — including the PCI-ID matching `load` now delegates to `DrvProbe` — is covered by the kernel-side tests §17 references.

---

## 21. Capability Delegation via `Exec`

`load` and `unload` both require capabilities (`SPAWN_DRIVER`, `UNLOAD_DRIVER`) that an ordinary Ring 3 program does not have. Before this feature existed, that was not a problem: driver-loading logic ran *inside* the shell's own process, and the shell is the one task marked privileged at boot (`kernel/src/main.rs`), so it sailed through `SpawnDriver`'s authorization check directly. Moving that logic into a separate `DRIVERS.BIN` process broke that shortcut — a process started via `Exec` is, and always was, unconditionally unprivileged (`process::exec_from_vfs`'s own doc comment), regardless of who started it.

The fix is a narrow delegation mechanism on `Exec` itself, not a way to make `DRIVERS.BIN` privileged. `Exec` gained a second argument, `requested_caps`, and the kernel computes what to actually grant with a small, pure, unit-tested function:

```rust
pub fn resolve_delegated_capabilities(
    caller_privileged: bool,
    caller_flags: Capabilities,
    requested: Capabilities,
) -> Capabilities {
    if caller_privileged {
        requested
    } else {
        caller_flags & requested
    }
}
```

A privileged caller (the shell) may delegate anything it asks for. An unprivileged caller may delegate at most the capabilities it already holds itself — the intersection of what it has and what it asks for — so a compromised or buggy unprivileged process can never manufacture a capability out of thin air and hand it to a child. When capabilities are granted, `syscall_exec_impl` attaches a `DriverCaps` block to the new task exactly the way `SpawnDriver` does for a driver it spawns (§6), just with `ResourceGrants::default()` — an empty MMIO grant, since `DRIVERS.BIN` itself never touches hardware directly; it only ever asks the kernel to do so on its behalf, on a device it never sees the address of.

Deciding *which* binary gets *which* capabilities is deliberately kept out of the kernel. The kernel only provides the delegation mechanism; the policy of "only `DRIVERS.BIN`, and only these three bits" lives entirely in the shell's `run_program()`, in a function just as small and just as directly tested:

```rust
fn requested_capabilities_for(name: &str) -> u64 {
    if name.eq_ignore_ascii_case("DRIVERS.BIN") {
        SPAWN_DRIVER | UNLOAD_DRIVER | LIST_DRIVERS
    } else {
        0
    }
}
```

Every other program the shell execs — `TUI.BIN`, `KBASIC.BIN`, `NETTOOLS.BIN`, a plain `hello.bin` — still gets exactly zero delegated capabilities, identical to `Exec`'s behavior before this feature existed. This is exactly why `net-tools.bin` (§14) never needs any capability at all: everything it does goes through `DrvLookup`/`NetSend`/`NetRecv`/`DrvQuery`, none of which check for `DriverCaps` on the caller's side. This split mirrors `SpawnDriver`'s own design: the kernel enforces a hard security *invariant* (a caller can never delegate more than it has), while a trusted, auditable piece of user-space code owns the *policy* of who the invariant actually applies to. Both the mechanism (`resolve_delegated_capabilities`) and the policy (`requested_capabilities_for`) are covered by their own host-level unit tests, and the full path — the shell delegates, `DRIVERS.BIN` receives exactly those three capabilities and nothing more, and every other exec'd program still receives none — is covered end-to-end by a dedicated kernel integration test suite (`kernel/tests/exec_capability_delegation_test.rs`).
