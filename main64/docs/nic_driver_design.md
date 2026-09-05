# NIC Driver Abstraction and Long-Term Driver Architecture

> Audience: coding agent tasked with implementing this design  
> Date: 2026-08-30  
> Related documents: `docs/drivers.md`, `docs/todo_drivers.md`

---

## 1. Problem Statement and Goals

### 1.1 Short-Term Goal (Phase 1)

The existing `rtl8139.bin` implementation contains the complete Ethernet/ARP/IPv4/ICMP stack
directly inside the same crate (`user_programs/rtl8139/src/net/`). New drivers for physical
network cards are to be added:

- **Intel 82577LM** — PCI ID `8086:10EA`  
- **Intel I219-V**   — PCI ID `8086:15B8`

**Constraint:** The entire Ethernet/ARP/IPv4/ICMP code must **not** be duplicated.

### 1.2 Long-Term Goal (Phase 2)

Device drivers shall run as **independent background processes** that can be loaded from the
shell via `load rtl8139.drv`. Ring-3 applications then communicate with the running driver
through a kernel IPC channel.

---

## 2. Overall Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  Ring 3                                                         │
│                                                                 │
│  Shell                    Application (e.g. ping.bin)           │
│  ┌──────────┐             ┌─────────────────────────────────┐   │
│  │load      │             │  lib_net_client                 │   │
│  │rtl8139   │             │  (NicClient::send / recv)       │   │
│  │.drv      │             └────────────┬────────────────────┘   │
│  └────┬─────┘                          │ NetSend / NetRecv       │
│       │ SpawnDriver (Syscall 35)        │ Syscalls (new)         │
│       │                                │                         │
│  ┌────▼──────────────────────────┐     │                         │
│  │  rtl8139.drv  (background)    │     │                         │
│  │  e1000e.drv   (background)    │◄────┘                         │
│  │  intel_nic.drv(background)    │                               │
│  │                               │                               │
│  │  ┌────────────────────────┐   │                               │
│  │  │  lib_net               │   │                               │
│  │  │  NetworkStack          │   │                               │
│  │  │  Ethernet/ARP/IPv4/ICMP│   │                               │
│  │  └────────────────────────┘   │                               │
│  └───────────────────────────────┘                               │
│                                                                 │
├──────────────────────── syscall boundary ───────────────────────┤
│  Ring 0  (Kernel)                                               │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Kernel IPC (shared-memory ring, Phase 2)                │   │
│  │  DriverRegistry: driver_id → task_id                     │   │
│  │  NetSend / NetRecv syscalls                              │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                 │
│  PCI · MMIO mapping · DMA · IRQ bridge (already present)       │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Phase 1 — Shared `lib_net` + Hardware Trait (short-term)

### 3.1 New Crate: `lib_net`

All code from `user_programs/rtl8139/src/net/` is **moved** (not copied) to `lib_net/src/`.
The `NicDevice` trait is added on top.

```
lib_net/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── nic.rs           ← NicDevice trait (hardware abstraction)
    ├── stack.rs         ← NetworkStack (unchanged from net/mod.rs)
    ├── config.rs        ← NetworkConfig
    ├── event.rs         ← NetworkEvent
    └── proto/
        ├── mod.rs
        ├── ethernet.rs  ← moved from net/ethernet.rs
        ├── arp.rs       ← moved from net/arp.rs
        ├── ipv4.rs      ← moved from net/ipv4.rs
        └── icmp.rs      ← moved from net/icmp.rs
```

**`lib_net/Cargo.toml`:**

```toml
[package]
name    = "lib_net"
version = "0.1.0"
edition = "2021"

[dependencies]
lib_driver = { path = "../lib_driver" }
```

**`lib_net/src/lib.rs`:**

```rust
#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod nic;
pub mod proto;
pub mod stack;
pub mod config;
pub mod event;

pub use config::NetworkConfig;
pub use event::NetworkEvent;
pub use nic::NicDevice;
pub use proto::arp::{ArpPacket, ArpTable, Ipv4Address};
pub use proto::ethernet::{EthernetFrame, MacAddress, ETHERNET_HEADER_LEN};
pub use proto::icmp::{IcmpEchoPacket, ICMP_HEADER_LEN};
pub use proto::ipv4::{Ipv4Packet, DEFAULT_TTL, IPV4_HEADER_MIN_LEN};
pub use stack::NetworkStack;
```

### 3.2 The `NicDevice` Trait

**`lib_net/src/nic.rs`:**

```rust
//! Hardware abstraction for a single Ethernet NIC.

use lib_driver::SysError;
use crate::proto::ethernet::MacAddress;

/// Unified interface for all Ethernet device drivers.
///
/// Implementations encapsulate all device-specific register operations,
/// DMA ring management, and interrupt handling. The NetworkStack is
/// generic over this trait so that Ethernet/ARP/IPv4/ICMP exist only once.
pub trait NicDevice {
    /// Returns the hardware-programmed MAC address read from the NIC.
    fn mac(&self) -> MacAddress;

    /// Transmits `packet` as a raw Ethernet frame.
    ///
    /// The caller guarantees `packet.len() >= 14`.
    /// The implementation pads to 60 bytes if required by the hardware.
    fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError>;

    /// Returns the next received Ethernet frame (non-blocking).
    ///
    /// Returns `Some(n)` if a packet was received, otherwise `None`.
    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize>;

    /// Disables TX/RX and releases hardware resources.
    fn shutdown(&mut self);
}
```

### 3.3 Migrating the Existing RTL8139 Driver

**Changes in `user_programs/rtl8139/`:**

1. Remove `src/net/`.
2. Add to `Cargo.toml`: `lib_net = { path = "../../lib_net" }`.
3. Add to `src/rtl8139.rs`:

```rust
use lib_net::nic::NicDevice;

impl NicDevice for Rtl8139Device {
    fn mac(&self) -> lib_net::proto::ethernet::MacAddress { self.mac() }

    fn transmit(&mut self, packet: &[u8]) -> Result<(), lib_driver::SysError> {
        self.transmit(packet)
    }

    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize> {
        self.poll_next_packet(out_buf)
    }

    fn shutdown(&mut self) { self.shutdown() }
}
```

4. In `src/main.rs` replace all `use net::…` with `use lib_net::…`.
5. Update the signatures of `execute_ping` / `execute_listen`:
   - `device: &mut Rtl8139Device` → `device: &mut impl NicDevice`

**The RTL8139 hardware code remains completely untouched.**

### 3.4 Intel NIC Driver Crate

A single crate for both Intel NICs — the register maps are identical:

```
user_programs/intel_nic/
├── Cargo.toml
├── link.ld              (identical to rtl8139/link.ld)
└── src/
    ├── main.rs          (PCI probe for 0x8086:0x10EA and 0x8086:0x15B8)
    └── intel_nic.rs     (IntelNicDevice + impl NicDevice)
```

**`Cargo.toml`:**

```toml
[package]
name    = "intel_nic"
version = "0.1.0"
edition = "2021"

[dependencies]
lib_driver = { path = "../../lib_driver" }
lib_kaos   = { path = "../../lib_kaos" }
lib_net    = { path = "../../lib_net" }

[[bin]]
name = "intel_nic"
path = "src/main.rs"
bench = false
```

#### 3.4.1 Hardware Architecture of the Intel e1000e / I219-V

Both NICs use a **descriptor ring** DMA model:

```
RX Descriptor Ring (N × 16 bytes):
  desc[i].buffer_addr  ← physical address of the 2048-byte RX payload buffer
  desc[i].status       ← hardware sets the DD bit (Descriptor Done) on receive

TX Descriptor Ring (N × 16 bytes):
  desc[i].buffer_addr  ← physical address of the packet to send
  desc[i].length       ← packet length in bytes
  desc[i].cmd          ← EOP(0x01) | IFCS(0x02) | RS(0x08)
  desc[i].status       ← hardware sets the DD bit after transmission
```

#### 3.4.2 Key MMIO Registers (BAR 0, 32-bit aligned)

| Register | Offset   | Description                           |
|----------|----------|---------------------------------------|
| CTRL     | `0x0000` | Device control (reset, link)          |
| STATUS   | `0x0008` | Device status                         |
| ICR      | `0x00C0` | Interrupt Cause Read (clear-on-read)  |
| IMS      | `0x00D0` | Interrupt Mask Set                    |
| IMC      | `0x00D8` | Interrupt Mask Clear                  |
| RCTL     | `0x0100` | Receive Control                       |
| TCTL     | `0x0400` | Transmit Control                      |
| RDBAL    | `0x2800` | RX Descriptor Base Address Low        |
| RDBAH    | `0x2804` | RX Descriptor Base Address High       |
| RDLEN    | `0x2808` | RX ring length (bytes)                |
| RDH      | `0x2810` | RX Descriptor Head                    |
| RDT      | `0x2818` | RX Descriptor Tail                    |
| TDBAL    | `0x3800` | TX Descriptor Base Address Low        |
| TDBAH    | `0x3804` | TX Descriptor Base Address High       |
| TDLEN    | `0x3808` | TX ring length (bytes)                |
| TDH      | `0x3810` | TX Descriptor Head                    |
| TDT      | `0x3818` | TX Descriptor Tail (kick register)    |
| RAL0     | `0x5400` | Receive Address Low (MAC bytes 0–3)   |
| RAH0     | `0x5404` | Receive Address High (MAC bytes 4–5)  |

#### 3.4.3 Descriptor Layout

```rust
/// 16-byte legacy RX descriptor (inside DmaBuffer; volatile access required).
#[repr(C, packed)]
struct RxDesc {
    buffer_addr: u64, // Physical address of the receive payload buffer.
    length:      u16, // Number of bytes written by hardware.
    checksum:    u16, // Unused.
    status:      u8,  // Bit 0 = DD (Descriptor Done), Bit 1 = EOP.
    errors:      u8,
    special:     u16,
}

/// 16-byte legacy TX descriptor (inside DmaBuffer; volatile access required).
#[repr(C, packed)]
struct TxDesc {
    buffer_addr: u64, // Physical address of the transmit payload buffer.
    length:      u16, // Packet length in bytes.
    cso:         u8,  // Checksum offset (0 = no offload).
    cmd:         u8,  // EOP(0x01) | IFCS(0x02) | RS(0x08).
    status:      u8,  // Bit 0 = DD (set by hardware after TX).
    css:         u8,  // Checksum start.
    special:     u16,
}
```

> **Mandatory for the implementing agent:** Every access to an RX or TX descriptor
> **must** use `core::ptr::read_volatile` / `core::ptr::write_volatile` and
> **must** include a `// SAFETY:` comment that explains why the access is safe.

#### 3.4.4 `IntelNicDevice` Struct

```rust
pub struct IntelNicDevice {
    model:      NicModel,    // E1000e (10EA) or E1000 (15B8)
    mmio:       Mmio,
    irq:        u8,
    mac:        MacAddress,

    _rx_descs:  DmaBuffer,   // N × 16 bytes of RxDesc
    _rx_bufs:   DmaBuffer,   // N × 2048 bytes of payload buffers
    rx_tail:    usize,

    _tx_descs:  DmaBuffer,   // N × 16 bytes of TxDesc
    _tx_bufs:   DmaBuffer,   // N × 2048 bytes of payload buffers
    tx_tail:    usize,

    rx_count:   usize,       // ring size (number of descriptors)
    tx_count:   usize,
}

pub enum NicModel { E1000e, E1000 }
```

#### 3.4.5 Initialization Sequence

```
Step 1  Reset: CTRL |= RST (bit 26); spin until RST = 0.
Step 2  Read MAC from RAL0/RAH0 (check bit 31 of RAH0 = valid bit).
Step 3  Allocate RX descriptor ring (N × 16 bytes DmaBuffer).
Step 4  Allocate N RX payload buffers (2048 bytes each), write buffer_addr into
        each descriptor, set status = 0.
Step 5  Write RDBAL/RDBAH with the ring physical address, RDLEN with ring size in bytes.
Step 6  Set RDH = 0, RDT = N - 1 (hand all descriptors to hardware).
Step 7  Enable RX: RCTL = EN | BAM (broadcast) | BSIZE=2048 | SECRC.
Step 8  Allocate TX descriptor ring (N × 16 bytes DmaBuffer).
Step 9  Allocate N TX payload buffers (2048 bytes each), write buffer_addr,
        set status = DD (software-owned initially).
Step 10 Write TDBAL/TDBAH, TDLEN; set TDH = TDT = 0.
Step 11 Enable TX: TCTL = EN | PSP | CT=0x10 | COLD=0x40.
Step 12 Subscribe IRQ (same pattern as RTL8139: irq::subscribe(irq)).
Step 13 Set IMS = RXT0 | TXDW | LSC to enable relevant interrupts.
```

#### 3.4.6 `poll_next_packet`

```
1. Read RxDesc at rx_tail via volatile pointer.
2. If status & DD == 0: return None (no packet ready).
3. Copy length bytes from the _rx_bufs[rx_tail] slot into out_buf.
4. Write status = 0 (release descriptor back to hardware).
5. Write RDT = rx_tail (return descriptor to hardware).
6. rx_tail = (rx_tail + 1) % rx_count.
7. Return Some(length).
```

#### 3.4.7 `transmit`

```
1. Check TxDesc at tx_tail: if status & DD == 0 → Err(Busy) (descriptor in use).
2. Copy packet bytes into the _tx_bufs[tx_tail] slot.
3. Write TxDesc: length = packet.len(), cmd = EOP|IFCS|RS, status = 0.
4. tx_tail = (tx_tail + 1) % tx_count.
5. Write TDT = tx_tail (kick hardware to start transmission).
6. Return Ok(()).
```

#### 3.4.8 Reading the MAC Address

```rust
let ral = mmio.read32(0x5400);
let rah = mmio.read32(0x5404);
// Bit 31 of RAH0 = valid bit; if not set, EEPROM fallback via EERD is required.
let mac = MacAddress([
    (ral & 0xFF)         as u8,
    ((ral >> 8)  & 0xFF) as u8,
    ((ral >> 16) & 0xFF) as u8,
    ((ral >> 24) & 0xFF) as u8,
    (rah & 0xFF)         as u8,
    ((rah >> 8)  & 0xFF) as u8,
]);
```

---

## 4. Phase 2 — Device Drivers as Independent Background Processes

### 4.1 Overview: What Changes

In Phase 1 the driver is started, blocks the shell until it exits (synchronous `wait`),
and communicates with nobody. In Phase 2 the driver runs permanently in the background;
Ring-3 applications send and receive packets through a kernel IPC channel.

```
Shell: load rtl8139.drv
  → SpawnDriver("rtl8139.drv", caps, grants)   [Syscall 35, no wait]
  → Driver runs as an independent task (TID X)
  → Kernel DriverRegistry: "nic:rtl8139" → TID X

ping.bin:
  → NicClient::open("nic:rtl8139")
  → NicClient::send(frame)    [NetSend syscall]
  → NicClient::recv(&mut buf) [NetRecv syscall, blocking/polling]
```

### 4.2 Shell Extension: `load <file.drv>`

**Changes in `user_programs/shell/src/main.rs`:**

```rust
"load" => {
    if let Some(file) = parts.next() {
        load_driver(file);
    } else {
        println!("Usage: load <name.drv>");
    }
}

fn load_driver(file: &str) {
    // Step 1: Match driver against PCI table and derive resource grants.
    let grants = derive_grants_from_pci(file);

    // Step 2: Spawn driver as a background process (no wait).
    let caps = Capabilities::MMIO | Capabilities::IRQ;
    match spawn_driver(file, caps.bits() as u64, grants.as_ref()) {
        Ok(tid) => println!("[shell] Driver '{}' started as TID {}", file, tid),
        Err(e)  => println!("[shell] Failed to load '{}': {:?}", file, e),
    }
}
```

> `spawn_driver` without a subsequent `wait` — the driver runs in the background.

### 4.3 Static Driver Table

In the shell code (and later in a dedicated driver manager):

```rust
const DRIVER_TABLE: &[(&str, u16, u16)] = &[
    ("rtl8139.drv",   0x10EC, 0x8139),  // Realtek RTL8139
    ("intel_nic.drv", 0x8086, 0x10EA),  // Intel 82577LM
    ("intel_nic.drv", 0x8086, 0x15B8),  // Intel I219-V
];
```

The function `derive_grants_from_pci(file)` searches this table and reads the matching
BARs and IRQ line from the PCI enumeration.

### 4.4 Kernel DriverRegistry

**New kernel data structure** in `kernel/src/drivers/registry.rs`:

```rust
/// Entry in the global driver registry.
pub struct DriverEntry {
    /// Unique name (e.g. "nic:rtl8139", "nic:intel_nic").
    pub name:      [u8; 32],
    /// Kernel task ID of the running driver process.
    pub tid:       usize,
    /// Shared-memory ring buffer (physical address and size).
    pub ring_phys: u64,
    pub ring_len:  usize,
}

/// Global driver registry (static array, up to 16 entries).
static DRIVER_REGISTRY: Mutex<[Option<DriverEntry>; 16]> = ...;
```

**New syscalls** for Phase 2:

| Nr | Name          | Direction            | Description                                               |
|----|---------------|----------------------|-----------------------------------------------------------|
| 39 | `DrvRegister` | Driver → Kernel      | Driver registers itself under a name                      |
| 40 | `DrvLookup`   | App → Kernel         | App resolves the TID of a registered driver               |
| 41 | `NetSend`     | App → Kernel → Driver| Send a packet to the driver (via shared-memory ring)      |
| 42 | `NetRecv`     | App → Kernel → Driver| Receive a packet from the driver (blocking / polling)     |

### 4.5 IPC Mechanism: Kernel-Mediated Shared-Memory Ring

The simplest safe approach: **the kernel owns one ring buffer per registered driver**;
applications read and write through syscalls. There is no direct shared memory between
Ring-3 processes (that would require unsafe cross-task pointers).

```
┌──────────────────────────────────────────────────────────┐
│  Ring buffer in kernel heap (physically contiguous)      │
│                                                          │
│  TX ring (App → Driver):                                 │
│    [head][slot 0][slot 1]…[slot N-1][tail]               │
│                                                          │
│  RX ring (Driver → App):                                 │
│    [head][slot 0][slot 1]…[slot N-1][tail]               │
│                                                          │
│  Slot format: [len: u16][data: [u8; 1536]]               │
└──────────────────────────────────────────────────────────┘
```

**`NetSend` flow (application sends a packet):**

```
1. App calls NetSend(driver_id, packet_ptr, packet_len).
2. Kernel validates: driver_id known? packet buffer within user canonical space?
3. Kernel copies packet into the TX ring slot (kernel-to-kernel copy).
4. Kernel wakes the waiting driver task (WaitQueue keyed by driver_id).
5. Syscall returns → app continues (fire-and-forget).
```

**Driver task flow (receives TX packets from apps):**

```
1. Driver calls DrvWaitTx(driver_id) — blocks on WaitQueue.
2. Kernel wakes it when a TX ring slot becomes available.
3. Driver reads the slot from the mapped ring (kernel maps the ring into the driver AS).
4. Driver sends the packet via NicDevice::transmit().
```

**`NetRecv` flow (application receives a packet):**

```
1. Driver receives a packet via NicDevice::poll_next_packet().
2. Driver writes the packet into the RX ring via DrvPushRx(driver_id, data).
3. Driver wakes waiting applications via the kernel WaitQueue.
4. App calls NetRecv(driver_id, buf, len) → kernel copies from the RX ring.
```

> **Implementation note:** Ring buffer and WaitQueue primitives already exist in
> similar form (IRQ bridge, scheduler). Phase 2 builds on these existing building
> blocks.

### 4.6 Driver-Side Adaptation (`main.rs` of a `.drv` process)

```rust
// Step 1: Initialize the hardware device (same as before).
let mut device = IntelNicDevice::init(mmio, irq)?;
let mut stack  = NetworkStack::new(device.mac());

// Step 2: Register driver in the kernel registry.
drv::register("nic:intel_nic").expect("registry full");

// Step 3: Main event loop (background process, runs forever).
loop {
    // TX: receive packets from applications and transmit them.
    if let Some((pkt, len)) = drv::poll_tx() {
        let _ = device.transmit(&pkt[..len]);
    }

    // RX: process received packets and forward them to waiting applications.
    let mut rx_buf = [0u8; 1536];
    while let Some(len) = device.poll_next_packet(&mut rx_buf) {
        let event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| {
            let _ = device.transmit(tx_pkt);
        });
        drv::push_rx(&rx_buf[..len]); // Write packet into the app RX ring.
        // Log events (ARP, ICMP, etc.) as before.
        let _ = event;
    }

    process::yield_now();
}
```

### 4.7 Application-Side `lib_net_client`

A new helper library for applications that communicate with a driver:

```
lib_net_client/
├── Cargo.toml
└── src/
    ├── lib.rs
    └── client.rs   ← NicClient (wraps NetSend/NetRecv syscalls)
```

```rust
pub struct NicClient {
    driver_id: u32,
}

impl NicClient {
    /// Opens a connection to the named driver.
    pub fn open(name: &str) -> Result<Self, SysError> { … }

    /// Sends a raw Ethernet frame.
    pub fn send(&self, frame: &[u8]) -> Result<(), SysError> { … }

    /// Receives a frame (blocking).
    pub fn recv(&self, buf: &mut [u8]) -> Result<usize, SysError> { … }
}
```

---

## 5. Workspace Changes

**`Cargo.toml` (workspace root):**

```toml
[workspace]
resolver = "2"
members = [
    "kaosldr_64",
    "kaosldr_uefi",
    "kernel",
    "lib_driver",
    "lib_kaos",
    "lib_net",              # NEW (Phase 1)
    "lib_net_client",       # NEW (Phase 2)
    "lib_tui",
    "user_programs/filedemo",
    "user_programs/exception_test",
    "user_programs/hello",
    "user_programs/kbasic",
    "user_programs/readline",
    "user_programs/rtl8139",
    "user_programs/intel_nic",   # NEW (Phase 1)
    "user_programs/shell",
    "user_programs/tui_app",
]
```

---

## 6. File Map

| Path | Status | Description |
|------|--------|-------------|
| `lib_net/` | **NEW** | Shared network stack + NicDevice trait |
| `lib_net/src/nic.rs` | **NEW** | `NicDevice` trait |
| `lib_net/src/proto/` | **MOVED** | Protocol modules from `rtl8139/src/net/` |
| `lib_net/src/stack.rs` | **MOVED** | `NetworkStack` from `rtl8139/src/net/mod.rs` |
| `lib_net_client/` | **NEW (Phase 2)** | Application-side NIC client library |
| `user_programs/rtl8139/src/net/` | **REMOVED** | Replaced by `lib_net` |
| `user_programs/rtl8139/src/rtl8139.rs` | **EXTENDED** | `+ impl NicDevice` |
| `user_programs/rtl8139/src/main.rs` | **MODIFIED** | `use lib_net::…` |
| `user_programs/intel_nic/` | **NEW** | Intel 82577LM + I219-V driver crate |
| `user_programs/shell/src/main.rs` | **EXTENDED** | Add `load` command |
| `kernel/src/drivers/registry.rs` | **NEW (Phase 2)** | Kernel DriverRegistry |
| `kernel/src/syscall/types.rs` | **EXTENDED (Phase 2)** | Syscalls 39–42 |
| `kernel/src/syscall/dispatch/driver.rs` | **EXTENDED (Phase 2)** | Handlers for 39–42 |
| `Cargo.toml` (workspace) | **MODIFIED** | `+lib_net`, `+intel_nic`, `+lib_net_client` |

---

## 7. Implementation Order

### Phase 1 (NIC abstraction — no kernel changes required)

1. **Create `lib_net/`** — move the `net/` directory from `rtl8139` verbatim;
   add the `NicDevice` trait in `nic.rs`; update `lib.rs`; add to the workspace.
2. **Migrate `rtl8139/`** — remove the local `net/` directory, add the `lib_net`
   dependency, add `impl NicDevice for Rtl8139Device`, update `use` paths in `main.rs`.
3. **Run `cargo test`** — all existing tests must pass without changes.
4. **Create `user_programs/intel_nic/`** — `Cargo.toml`, `link.ld` (copy from `rtl8139`),
   skeleton `main.rs` with PCI probe for `0x10EA` / `0x15B8`.
5. **Implement `intel_nic.rs`** — `IntelNicDevice::init` (steps 1–13 from §3.4.5),
   `poll_next_packet` (§3.4.6), `transmit` (§3.4.7), `shutdown`.
6. **Wire `impl NicDevice for IntelNicDevice`**.
7. **CLI in `intel_nic/src/main.rs`** — identical to the `rtl8139/main.rs` CLI;
   update the device type, PCI IDs, and prompt string.
8. **Run `cargo test`**, **`cargo fmt --check`**, **`cargo clippy`** — all must pass.

### Phase 2 (background drivers — kernel extensions required)

9. **Shell command `load`** — extend `user_programs/shell/src/main.rs`.
10. **`rtl8139.drv` / `intel_nic.drv`** — rename existing binaries (FAT32: 8.3 name).
    Change `run_rtl8139_driver()` to call `spawn_driver` without `wait`.
11. **Kernel DriverRegistry** — `kernel/src/drivers/registry.rs`.
12. **Syscalls 39–42** — `DrvRegister`, `DrvLookup`, `NetSend`, `NetRecv` in
    `kernel/src/syscall/types.rs` and `dispatch/driver.rs`.
13. **Adapt driver event loop** (§4.6 — background mode with ring buffer).
14. **Create `lib_net_client/`** for applications.
15. **Run `cargo test`**, **`cargo fmt --check`**, **`cargo clippy`** — all must pass.

---

## 8. Testing Strategy

### 8.1 Unit Tests (host, `cargo test`)

All protocol-level tests remain in `lib_net`:

- `lib_net::proto::ethernet` — frame serialization/parsing (migrated from `rtl8139`)
- `lib_net::proto::arp` — packet serialization, ARP table lookup/update
- `lib_net::proto::ipv4` — header serialization, checksum
- `lib_net::proto::icmp` — echo request/reply round-trip
- `lib_net::stack` — `handle_rx_packet` with a mock `NicDevice`

### 8.2 Mock `NicDevice` for Stack Tests

```rust
#[cfg(test)]
struct MockNic {
    rx_queue: alloc::collections::VecDeque<alloc::vec::Vec<u8>>,
    tx_log:   alloc::vec::Vec<alloc::vec::Vec<u8>>,
    mac:      MacAddress,
}

#[cfg(test)]
impl NicDevice for MockNic {
    fn mac(&self) -> MacAddress { self.mac }

    fn transmit(&mut self, pkt: &[u8]) -> Result<(), lib_driver::SysError> {
        self.tx_log.push(pkt.to_vec());
        Ok(())
    }

    fn poll_next_packet(&mut self, out: &mut [u8]) -> Option<usize> {
        let pkt = self.rx_queue.pop_front()?;
        let n = pkt.len().min(out.len());
        out[..n].copy_from_slice(&pkt[..n]);
        Some(n)
    }

    fn shutdown(&mut self) {}
}
```

### 8.3 Integration Tests on Physical Hardware

- Boot KAOS on a machine equipped with an Intel 82577LM or I219-V.
- Run `load intel_nic.drv` from the shell.
- `ifconfig` shows the correct hardware MAC address.
- `ping <gateway>` receives ICMP Echo Replies.
- `listen` correctly answers ARP requests.

---

## 9. Constraints and Non-Goals

| Constraint | Rationale |
|------------|-----------|
| No external crates | Project rules (`AGENTS.md`) |
| `#![no_std]` in `lib_net` | Used by `no_std` driver processes |
| `alloc` is permitted | `ArpTable` uses `Vec`; allocator is present |
| No TCP/UDP | Out of scope; ICMP ping is the feature target |
| No interrupt-driven RX | Polling model, consistent with RTL8139 |
| No IOMMU/DMA protection | Known limitation (`todo_drivers.md §3.5`) |
| No MSI/MSI-X | Legacy PIC interrupts, same as RTL8139 |
| FAT32 8.3 filenames | Driver binaries: `RTL8139.DRV`, `INTLNIC.DRV` |
