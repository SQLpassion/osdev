# KAOS Rust Kernel: PCI Subsystem Deep Technical Documentation

This document explains the Peripheral Component Interconnect (PCI) bus driver implementation in the KAOS Rust kernel. It details the PCI Configuration Space access mechanisms, Base Address Register (BAR) sizing algorithms, device scanning policies, human-readable string mapping helpers, and the shell-accessible REPL query interface.

Target audience: kernel developers who need exact architectural behavior and implementation constraints for building PCI-based hardware drivers (such as AHCI storage and Ethernet networking).

---

## 1. Architectural Foundations of PCI

PCI is a local computer bus for attaching hardware devices to a computer's motherboard. In x86_64 systems, standard PCI-compliant devices expose a standardized 256-byte register set known as the **PCI Configuration Space**.

The kernel accesses the configuration space of any device using Port I/O through two specific x86 register ports:

- `CONFIG_ADDRESS` (Port `0xCF8`, 32-bit write-only): Used to specify the target bus, device, function, and register offset.
- `CONFIG_DATA` (Port `0xCFC`, 32-bit read/write): Used to transmit or receive the 32-bit data to/from the selected configuration register.

---

## 2. Mental Model

Use this mental model:

- **Bus-Device-Function (BDF) Addressing**: Every device on the PCI bus is addressed by a unique BDF tuple.
  - Bus: 8-bit number (0..=255)
  - Device/Slot: 5-bit number (0..=31)
  - Function: 3-bit number (0..=7)
- **Scanning**: A brute-force iteration scans BDF combinations. Devices are cached in a thread-safe global list at boot.
- **Dynamic Translation**: Raw Vendor IDs, Device IDs, and Class/Subclass codes are translated via lightweight in-tree lookup tables into human-readable strings.
- **BAR Sizing**: Temporary writing of all 1s to Base Address Registers (BARs) query hardware address requirements without hardcoding resource allocation.
- **REPL Command**: Queries the cached list and renders BDF addresses, vendor identifiers, device classes, and memory/port allocations.

---

## 3. PCI Configuration Address Layout

To read or write to a specific BDF and register offset, the kernel constructs a 32-bit address word and writes it to `CONFIG_ADDRESS` (Port `0xCF8`). The format of this address word is:

```
 31 30      24 23      16 15      11 10       8 7         2 1 0
+--+----------+----------+----------+----------+-----------+---+
|E | Reserved |   Bus    |  Device  | Function | Register  | 0 |
+--+----------+----------+----------+----------+-----------+---+
```

- **Bit 31 (E - Enable)**: Must be set to `1` to enable translation.
- **Bits 30-24**: Reserved, must be `0`.
- **Bits 23-16**: Bus Number (`0..=255`).
- **Bits 15-11**: Device/Slot Number (`0..=31`).
- **Bits 10-8**: Function Number (`0..=7`).
- **Bits 7-2**: Double-Word Offset (`0..=63`, corresponding to 256-byte configuration space divided by 4).
- **Bits 1-0**: Hardwired to `00`.

Once this address is written, the CPU can read or write 32-bit values from `CONFIG_DATA` (Port `0xCFC`) to access the selected register.

---

## 4. The PCI Configuration Space Layout (Header Type 0x00)

The first 64 bytes of the PCI configuration space are standardized and structured according to the device's header type. The most common layout, used for standard endpoints, is **Header Type 0x00**:

```
 31                     16 15                      0  Register Offset
+-------------------------+-------------------------+
|        Device ID        |        Vendor ID        | 0x00 (DWord 0)
+-------------------------+-------------------------+
|         Status          |         Command         | 0x04 (DWord 1)
+------------+------------+------------+------------+
| Class Code |  Subclass  |  Prog IF   |Revision ID | 0x08 (DWord 2)
+------------+------------+------------+------------+
|    BIST    |Header Type |Latency Tmr |Cache Line S| 0x0C (DWord 3)
+------------+------------+------------+------------+
|                  Base Address Register 0          | 0x10 (DWord 4)
+---------------------------------------------------+
|                  Base Address Register 1          | 0x14 (DWord 5)
+---------------------------------------------------+
|                  Base Address Register 2          | 0x18 (DWord 6)
+---------------------------------------------------+
|                  Base Address Register 3          | 0x1C (DWord 7)
+---------------------------------------------------+
|                  Base Address Register 4          | 0x20 (DWord 8)
+---------------------------------------------------+
|                  Base Address Register 5          | 0x24 (DWord 9)
+---------------------------------------------------+
|               CardBus CIS Pointer                 | 0x28 (DWord 10)
+-------------------------+-------------------------+
|      Subsystem ID       |   Subsystem Vendor ID   | 0x2C (DWord 11)
+-------------------------+-------------------------+
|             Expansion ROM Base Address            | 0x30 (DWord 12)
+-------------------------+------------+------------+
|        Reserved         |  Reserved  | Capabilities| 0x34 (DWord 13)
+-------------------------+------------+------------+
|                      Reserved                     | 0x38 (DWord 14)
+------------+------------+------------+------------+
|  Max Lat.  |  Min Gnt.  |Interrupt Pin|Interrupt Ln| 0x3C (DWord 15)
+------------+------------+------------+------------+
```

---

## 5. Detailed Breakdown of Configuration Space Fields

To understand what the BDF metadata fields represent, let's look at each register offset and its function:

### 5.1 Vendor ID & Device ID (Offset `0x00`)
- **Vendor ID (16-bit)**: A unique value assigned by the PCI-SIG (PCI Special Interest Group) that identifies the manufacturer of the physical silicon chip.
  - `0x8086`: Intel Corporation
  - `0x10EC`: Realtek Semiconductor Co., Ltd.
  - `0x10DE`: NVIDIA Corporation
  - `0x1234`: Bochs/QEMU (emulated VGA device)
  - `0xFFFF` / `0x0000`: Used as a sentinel to denote a **non-existent device** (bus line pulled high, returning all 1s).
- **Device ID (16-bit)**: A value assigned by the manufacturer to uniquely identify the specific device model family.
  - `0x100E`: Intel 82540EM Gigabit Ethernet Controller (e1000)
  - `0x1111`: Bochs/QEMU VGA Controller
  - `0x7010`: Intel 82371SB PIIX3 IDE Interface

### 5.2 Command & Status (Offset `0x04`)
- **Command (16-bit)**: Provides control over a device's ability to generate and respond to PCI cycles. For example, setting Bit 0 enables Response to I/O Space, while setting Bit 1 enables Response to Memory Space (MMIO).
- **Status (16-bit)**: Records status information for PCI bus-related events.

### 5.3 Class Code, Subclass, Prog IF & Revision ID (Offset `0x08`)
- **Class Code (8-bit)**: Indicates the general class of device function. Used for generic driver lookup.
  - `0x00`: Unclassified device
  - `0x01`: Mass Storage Controller
  - `0x02`: Network Controller
  - `0x03`: Display Controller
  - `0x06`: Bridge Device
- **Subclass Code (8-bit)**: Refines the Class Code to a specific category.
  - If Class is `0x01` (Mass Storage):
    - `0x01`: IDE Interface
    - `0x06`: SATA Controller
  - If Class is `0x02` (Network):
    - `0x00`: Ethernet Controller
  - If Class is `0x06` (Bridge):
    - `0x00`: Host Bridge (connects CPU to PCI bus)
    - `0x01`: ISA Bridge (connects PCI bus to ISA bus)
- **Programming Interface / Prog IF (8-bit)**: Specifies the register-level protocol or programming interface of the device subclass.
  - Under SATA Controller (`0x01:0x06`):
    - `0x01`: Advanced Host Controller Interface (**AHCI**). This is what enables the kernel to recognize standard AHCI controllers dynamically without vendor-specific logic.
- **Revision ID (8-bit)**: Specifies the hardware revision number. Useful for drivers to apply chip-specific workarounds (errata).

### 5.4 Header Type (Offset `0x0E`)
- Identifies the layout of DWords 4-15 in the configuration header and also indicates multi-function support:
  - Bit 7: Multi-Function Flag. If set to `1` (e.g., `header_type & 0x80 != 0`), the physical device comprises multiple functional blocks (e.g., function 0 is IDE, function 3 is ACPI). If set to `0`, the device is a single-function device and we can skip scanning functions 1-7.
  - Bits 6-0: Header Layout.
    - `0x00`: Standard Device (Endpoint)
    - `0x01`: PCI-to-PCI Bridge
    - `0x02`: CardBus Bridge

### 5.5 Interrupt Line & Interrupt Pin (Offset `0x3C`)
- **Interrupt Line (8-bit)**: Identifies which system interrupt vector the device's interrupt pin is wired to. The BIOS/firmware writes this during boot. The OS driver reads this value to register its interrupt handler (ISR) with the IDT.
- **Interrupt Pin (8-bit)**: Read-only value indicating which physical interrupt pin is routed on the PCI board:
  - `0x01`: INTA#
  - `0x02`: INTB#
  - `0x03`: INTC#
  - `0x04`: INTD#

---

## 6. Base Address Registers (BARs)

PCI devices use Base Address Registers (BARs) to request address space for their registers or memory. Standard PCI devices (Header Type `0x00`) have up to 6 BARs starting at register offset `0x10`.

There are two primary types of BARs, distinguished by the lowest bit (Bit 0):

```
I/O Space BAR Layout:
 31                                               2 1 0
+--------------------------------------------------+---+
|                Base Port Address                 |R|1|
+--------------------------------------------------+---+

Memory Space BAR Layout:
 31                                       4 3 2   1   0
+------------------------------------------+---+-----+---+
|              Base Physical Address       | P |Type | 0 |
+------------------------------------------+---+-----+---+
```

- **I/O Space BAR (Bit 0 = 1)**: Maps registers to Port I/O space.
  - Bit 1: Reserved.
  - Bits 31-2: Base Port Address.
- **Memory Space BAR (Bit 0 = 0)**: Maps registers to Physical Memory Space (MMIO).
  - Bits 2-1 (Type):
    - `00`: 32-bit BAR (address anywhere in 32-bit physical space).
    - `10`: 64-bit BAR (address anywhere in 64-bit physical space).
  - Bit 3 (P - Prefetchable): Indicates if reads are side-effect-free.
  - Bits 31-4: Base Physical Address.

### 6.1 64-bit Memory BARs
When type bits are `10`, the BAR is a 64-bit Memory BAR. In this configuration, the current BAR contains the lower 32 bits of the address, and the next adjacent BAR (e.g. `BAR + 1`) contains the upper 32 bits. When iterating through BARs, the driver must combine both registers and **skip the next index** during scanning to prevent double-processing.

### 6.2 BAR Sizing Algorithm
At boot, BAR register values hold physical base addresses, but their size requirements are unknown. The kernel determines the size of each BAR dynamically using the following specification-compliant algorithm:

```
Step 1: Read and store the original BAR value.
Step 2: Write all 1s (0xFFFFFFFF) to the BAR register.
Step 3: Read back the BAR register. Hardware sets unchangeable bits to 0.
Step 4: Restore the original BAR value immediately.
Step 5: Mask off status bits (lower 4 bits for Memory, lower 2 bits for I/O).
Step 6: Invert the mask, add 1 to compute the size in bytes: size = (!mask + 1).
```

---

## 7. Dynamic Device Discovery and Query APIs

Instead of hardcoding the physical addresses or bus numbers of peripherals, the KAOS Rust kernel performs a brute-force scan of the entire BDF address space during boot and caches the structures. Device drivers later query the cached list dynamically.

### 7.1 The Bus Scan Flow (`pci::init()`)
The bus scanner executes in early boot:

```
Step 1: Clear the global PCI_DEVICES vector to ensure idempotency.
Step 2: Loop 'bus' from 0 to 255.
Step 3: Loop 'slot' from 0 to 31.
Step 4: Read Vendor ID from function 0 (Offset 0x00).
        If Vendor ID is 0xFFFF or 0x0000, no physical card resides in this slot.
        Skip immediately to the next slot (huge speedup!).
Step 5: Read Header Type (Offset 0x0E).
        If (header_type & 0x80) != 0, it is a multi-function device. We must loop 'func' from 0 to 7.
        Otherwise, it is a single-function card. We only process 'func = 0'.
Step 6: For each active function BDF:
        - Read and parse Vendor ID, Device ID, Class, Subclass, Prog IF, Revision ID, Interrupt Pin, Interrupt Line.
        - Parse and size BARs 0 to 5.
        - Skip the adjacent register if a 64-bit Memory BAR is encountered.
Step 7: Push the fully hydrated PciDevice struct to the static thread-safe PCI_DEVICES vector.
```

### 7.2 Thread-Safe Global Storage (`PCI_DEVICES`)
To prevent concurrent read/write race conditions during hotplugging or multi-core execution, the list is wrapped in a thread-safe SpinLock:
```rust
static PCI_DEVICES: SpinLock<Vec<PciDevice>> = SpinLock::new(Vec::new());
```

---

## 8. Human-Readable String Mapping

To make debug logs and console command reports intuitive for kernel developers, the kernel implements lightweight, in-tree static mapping functions:

- `pci::vendor_to_str(vendor_id) -> &'static str`: Resolves the silicon manufacturer (e.g. `0x8086` -> `"Intel Corporation"`).
- `pci::device_to_str(vendor_id, device_id) -> &'static str`: Resolves the specific hardware controller name (e.g. `0x100E` -> `"82540EM Gigabit Ethernet Controller"`).
- `pci::class_to_str(class_code, subclass) -> &'static str`: Resolves the standardized peripheral class name (e.g. `0x02`, `0x00` -> `"Ethernet Controller"`).

---

## 9. Driver Integration (Querying Devices)

Drivers utilize specific lookup APIs to bind themselves to discovered devices:

### 9.1 Lookup by Vendor & Device ID (`pci::find_device`)
Used when a driver is tailored to a very specific chip model (vendor-specific driver):
```rust
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices.iter().find(|d| d.vendor_id == vendor_id && d.device_id == device_id).cloned()
}
```
**Example Application**:
An Intel e1000 driver starts up and queries the PCI bus:
```rust
if let Some(nic) = pci::find_device(0x8086, 0x100E) {
    debugln!("e1000 Network Card found at BDF {:02x}:{:02x}.{}", nic.bus, nic.device, nic.function);
    // NIC BAR 0 is Memory Map base address
    if let BarType::Memory32 { address, size, .. } = nic.bars[0].bar_type {
        // Map physical address and initialize e1000 registers
    }
}
```

### 9.2 Lookup by Class & Subclass (`pci::find_by_class`)
Used when a driver is a standard specification driver (generic driver lookup):
```rust
pub fn find_by_class(class_code: u8, subclass: u8) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    devices.iter().find(|d| d.class_code == class_code && d.subclass == subclass).cloned()
}
```
**Example Application**:
A generic **AHCI storage driver** queries the PCI bus to find any compliant SATA controller, regardless of whether it is manufactured by Intel, AMD, or ASMedia:
```rust
if let Some(sata_ctrl) = pci::find_by_class(0x01, 0x06) {
    debugln!("AHCI-compliant SATA controller found at BDF {:02x}:{:02x}.{}", sata_ctrl.bus, sata_ctrl.device, sata_ctrl.function);
    // Parse AHCI HBA capability registers...
}
```

---

## 10. REPL Shell Command

Rather than flooding the screen during boot, PCI information is exposed interactively in the REPL (Read-Eval-Print Loop) shell via the `pci` command.

When the user enters `pci` in the shell, the console formats BDF locations and translates IDs into descriptive textual names.

### 10.1 Real-World Console Output Example
When booted in QEMU, the shell output displays:

```text
--- PCI Bus Scan (6 devices found) ---
PCI 00:00.0 | Intel Corporation (8086): 430FX - 82437FX System Controller [Triton I] (1237)
  Class: Host Bridge (06:00) | IRQ: 0
PCI 00:01.0 | Intel Corporation (8086): 82371SB PIIX3 ISA [Triton II] (7000)
  Class: ISA Bridge (06:01) | IRQ: 0
PCI 00:01.1 | Intel Corporation (8086): 82371SB PIIX3 IDE [Triton II] (7010)
  Class: IDE Interface (01:01) | IRQ: 0
  BAR 4: I/O Port 0xc040 (size 16)
PCI 00:01.3 | Intel Corporation (8086): 82371AB/EB/MB PIIX4 ACPI (7113)
  Class: Bridge Device (06:80) | IRQ: 9
PCI 00:02.0 | QEMU/Bochs (1234): Bochs/QEMU VGA Card (1111)
  Class: VGA Compatible Controller (03:00) | IRQ: 0
  BAR 0: 32-bit Mem 0xfd000000 (size 16777216, pref: true)
  BAR 2: 32-bit Mem 0xfebf0000 (size 4096, pref: false)
PCI 00:03.0 | Intel Corporation (8086): 82540EM Gigabit Ethernet Controller (100e)
  Class: Ethernet Controller (02:00) | IRQ: 11
  BAR 0: 32-bit Mem 0xfebc0000 (size 131072, pref: false)
  BAR 1: I/O Port 0xc000 (size 64)
```

---

## 11. Boot-Time Initialization Sequence

The PCI subsystem scans the PCI bus during early kernel boot after the Heap Manager is ready, but remains silent:

```
1. serial::init()         <- Early serial debug output
2. gdt::init()            <- Set up kernel segments & GDT
3. fpu::init()            <- Initialize FPU/SSE template
4. pmm::init()            <- Initialize Physical Memory Manager
5. interrupts::init()     <- Set up IDT/PIC
6. vmm::init()            <- Enable Virtual Paging
7. heap::init()           <- Initialize Kernel Heap allocator
8. pci::init()            <- Perform silent PCI brute force bus scan (populate cache)
9. drivers::ata::init()   <- Initialize primary ATA PIO driver
10. io::fat12::init()     <- Read root directory from disk
11. scheduler::init()     <- Ready kernel tasks (worker, REPL)
12. interrupts::enable()  <- Start multitasking timer tick
```

---

## 12. ASCII Overview of the PCI Subsystem

```
+--------------------+
| early boot scan    |
| (pci::init)        |
+--------------------+
          |
          v   reads configuration space
+------------------------------------+
| writes address -> Port 0xCF8       |
| reads data     <- Port 0xCFC       |
+------------------------------------+
          |
          v   populates
+-----------------------------------------+
| SpinLock<Vec<PciDevice>> (PCI_DEVICES)  |
+-----------------------------------------+
          |
          +-----------------------+
          |                       |
          v                       v
+--------------------+  +--------------------+
| REPL command "pci" |  | Future Drivers     |
| writes BDF/BARs    |  | (AHCI, e1000)      |
| to VGA Screen      |  | query matching class|
+--------------------+  +--------------------+
```
