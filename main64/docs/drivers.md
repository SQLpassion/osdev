# User-Space Drivers in KAOS: From a PCI Device to a Network Stack

> Status: branch `feature/drivers`, August 29, 2026  
> Audience: readers with no previous experience in driver, kernel, or network development

This document explains the driver architecture implemented on `feature/drivers` from first principles. Its purpose is not merely to describe **what** the code does, but to explain **why** each layer exists and how data, permissions, and hardware events move through the system. The concrete example is a Realtek RTL8139 network driver that runs as an ordinary Ring 3 process while retaining controlled access to PCI hardware, MMIO registers, DMA memory, and interrupts.

The text follows the real lifetime of the driver. It begins with the necessary hardware and operating-system concepts, follows the driver from the shell through the new kernel syscalls, and then examines device initialization and the transmit and receive paths. The final sections explain the build and test infrastructure and the current technical limitations. The protocol stack the driver hands frames to — Ethernet, ARP, IPv4, and ICMP — is documented separately in [`docs/networking.md`](networking.md), since it is hardware-agnostic and lives in its own crate, `lib_net`.

The most important implementation files are:

* [`kernel/src/process/capabilities.rs`](../kernel/src/process/capabilities.rs)
* [`kernel/src/syscall/dispatch/driver.rs`](../kernel/src/syscall/dispatch/driver.rs)
* [`kernel/src/drivers/irq_bridge.rs`](../kernel/src/drivers/irq_bridge.rs)
* [`lib_driver`](../lib_driver/src/lib.rs)
* [`drivers/rtl8139/src/rtl8139.rs`](../drivers/rtl8139/src/rtl8139.rs)
* [`drivers/rtl8139/src/main.rs`](../drivers/rtl8139/src/main.rs)

---

## 1. Why a Driver Needs Special Privileges

An ordinary application works with abstractions. It opens a file, writes text to a console, or asks the operating system for memory. It does not need to know which PCI bus contains a storage controller or which register bit starts a transmission. A device driver sits exactly at that boundary. It translates an abstract request such as “send this Ethernet frame” into register accesses and memory operations understood by a particular device.

This work requires capabilities that normal processes must not possess. A network driver has to read and write the registers of a PCI card. It has to prepare memory that both the CPU and the device can access. It may also have to react to asynchronous hardware interrupts. If every process could perform these operations without restriction, a bug or malicious program could reconfigure unrelated devices, overwrite foreign memory, or freeze the entire machine.

x86 therefore separates execution into privilege levels. KAOS, like many operating systems, primarily uses Ring 0 and Ring 3. The kernel executes in Ring 0 and may run privileged CPU instructions, modify page tables, and program interrupt controllers. Applications execute in Ring 3. When they attempt a forbidden operation, the CPU raises an exception instead of carrying it out.

A traditional monolithic kernel also runs drivers in Ring 0. This is fast, but a bad pointer inside a driver can corrupt the kernel. The branch `feature/drivers` chooses a different architecture: the RTL8139 driver is a normal process with its own virtual address space. The kernel retains ownership of privileged mechanisms and exposes only small, validated interfaces.

The resulting model can be summarized as follows:

```text
Ring 3

  Shell / driver manager              RTL8139 driver process
  discovers the PCI device            operates only that device
             |                                  |
             | SpawnDriver                      | MapPhysical / AllocDma
             | plus exact grants                | IrqSubscribe / IrqWait
             v                                  v
---------------------------- syscall boundary ----------------------------
Ring 0

  capability checks -> paging -> PMM -> IRQ bridge -> hardware
```

The essential idea is that a driver does not receive unrestricted “hardware access.” It receives authority over the exact resources belonging to its device.

---

## 2. The Four Hardware Concepts Behind the Branch

Four concepts carry almost the entire implementation: PCI, BAR/MMIO, DMA, and IRQ. They describe different aspects of communication between the CPU and a device.

### 2.1 PCI and Device Identity

PCI is a standardized bus for attaching devices. Every PCI device exposes a configuration space containing, among other fields, a vendor identifier, a device identifier, a device class, interrupt information, and Base Address Registers. The RTL8139 is recognized by the combination of vendor ID `0x10EC` and device ID `0x8139`.

A PCI function is addressed by a bus, device, and function tuple. For example, `00:03.0` means bus 0, device 3, function 0. KAOS scans these addresses during boot and stores the devices it finds. The user-space driver later queries that cached list through the existing PCI syscalls.

The new code enables three bits in the PCI Command Register during enumeration:

```rust
let orig_cmd = unsafe { pci_config_read(bus, slot, func, 0x04) };
let new_cmd = (orig_cmd & 0xFFFF_0000)
    | ((orig_cmd & 0x0000_FFFF) | 0x0007);
unsafe { pci_config_write(bus, slot, func, 0x04, new_cmd) };
```

The lowest three bits enable I/O Space, Memory Space, and Bus Mastering. Memory Space is necessary before the device responds to MMIO accesses. Bus Mastering allows the device to initiate DMA transactions. The implementation currently enables these bits for every discovered PCI device, not only the RTL8139.

The change is located in [`kernel/src/drivers/pci/mod.rs`](../kernel/src/drivers/pci/mod.rs). The existing [`docs/pci.md`](pci.md) provides a deeper explanation of PCI configuration-space access and BAR discovery.

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

If the physical BAR begins at `0xFEB0_0000`, then `REG_CHIPCMD = 0x37` means that the command register is physically accessible at `0xFEB0_0037`. A Ring 3 process cannot dereference that physical address directly. The kernel must first map it into the process's virtual address space under controlled conditions.

MMIO accesses must also be volatile. For ordinary memory, a compiler may assume that two reads return the same value if visible program code did not write between them. A hardware register can change independently. `read_volatile` and `write_volatile` tell the compiler that every individual access must really occur.

### 2.3 DMA

Direct Memory Access means that a device reads or writes main memory directly. To transmit a packet, the CPU places an Ethernet frame into RAM and tells the network card its physical address. The card reads that buffer and sends the bytes. During reception, the card writes incoming frames into a prepared RAM region.

This introduces an important distinction. A driver uses virtual addresses because each process owns a separate virtual address space. The PCI device in this system does not understand process page tables. It requires physical addresses. A DMA buffer therefore has two addresses:

```text
The driver sees:   virtual address  0x00007800_00001000
The page table:    maps that address to a RAM frame
The device sees:   physical address 0x00000000_01234000
```

The RTL8139 also requires physically contiguous memory. Several consecutive virtual pages can normally point to unrelated physical frames. The branch therefore adds a PMM allocation routine that searches for a contiguous sequence of free physical frames.

### 2.4 IRQs and EOI

An Interrupt Request, or IRQ, is an asynchronous signal from a device to the CPU. Instead of continuously inspecting a register, the CPU can perform other work and be interrupted when the device needs attention. The legacy PIC used by this system manages 16 IRQ lines, mapped to IDT vectors 32 through 47.

After an interrupt has been serviced, software must issue an End Of Interrupt, or EOI, to tell the PIC that the line may be used again. A user-space driver cannot issue this EOI immediately in the first Ring 0 handler. The Ring 3 process must first inspect and acknowledge the interrupt reason in the device itself. Otherwise the device may continue asserting the line and immediately trigger another interrupt.

---

## 3. The Complete RTL8139 Startup Path

The driver is not launched like an ordinary program. The shell acts as a small driver manager. Its `run_rtl8139_driver()` function in [`user_programs/shell/src/main.rs`](../user_programs/shell/src/main.rs) queries the PCI device list and searches for `0x10EC:0x8139`.

The shell uses that scan **only** to decide whether the card is present, so it can print a helpful message if it is not:

```rust
if !pci_device_present(&[(0x10EC, 0x8139)]) {
    println!("[shell] Error: No Realtek RTL8139 network card (10EC:8139) found on PCI bus.");
    return;
}
```

The shell deliberately does **not** compute the resource grants. It runs in Ring 3 as an ordinary unprivileged process, so any address range it named would be an address range chosen by untrusted code — and `MapPhysical` would then map it. A grant a caller can pick for itself is not a security boundary at all: a caller could name a kernel physical frame instead of a device BAR and read or write it from Ring 3. The kernel therefore derives every grant itself, from its own PCI enumeration, in [`kernel/src/drivers/driver_db.rs`](../kernel/src/drivers/driver_db.rs). The binary name selects which device the driver may bind to; it never selects an address.

The shell then starts the driver with the `MMIO | IRQ` capability bits and no grant request at all:

```rust
let caps = 1 | 2; // MMIO (1) | IRQ (2)

match spawn_driver("rtl8139.bin", caps, None) {
    Ok(pid) => {
        let _ = process::wait(pid as usize);
    }
    Err(err) => {
        println!("Failed to spawn RTL8139 driver: {:?}", err);
    }
}
```

The `UserDriverGrants` structure still exists in the ABI, but its meaning is now a *request* rather than a grant. A caller that passes one is asking the kernel to confirm that a particular BAR base and IRQ belong to the device the driver was bound to; a mismatch is rejected with `PermissionDenied` instead of being silently accepted. The value `0xFF` in `irq` and `0` in `mmio_base` mean "no preference". Explicit padding guarantees that both sides of the syscall boundary see a structure that is exactly 24 bytes long and aligned to eight bytes. Without a stable ABI layout, the compiler could arrange fields differently, causing the kernel and user process to interpret the same bytes in incompatible ways.

The shell waits in the foreground until the driver exits. Commands such as `rtl8139`, `rtl8139.bin`, `driver rtl8139`, and `exec rtl8139` intentionally lead to this path. A normal `exec` would create a process without `DriverCaps`, so its first attempt to map the BAR would fail with `PermissionDenied`.

In user space, [`lib_driver/src/spawn.rs`](../lib_driver/src/spawn.rs) prepares the call. It copies the file name into a local 128-byte buffer and adds a null terminator:

```rust
let mut buf = [0u8; 128];
let name_bytes = name.as_bytes();

if name_bytes.len() >= 128 {
    return Err(SysError::InvalidArgument);
}

buf[..name_bytes.len()].copy_from_slice(name_bytes);
buf[name_bytes.len()] = 0;
```

The library then executes `int 0x80` with syscall number 35. The kernel receives the file-name pointer in `RDI`, the capability bits in `RSI`, and the `UserDriverGrants` pointer in `RDX`.

---

## 4. How Capabilities and Resource Grants Work Together

The security model is implemented in [`kernel/src/process/capabilities.rs`](../kernel/src/process/capabilities.rs). A capability describes a class of privileged operations. A resource grant identifies the concrete resource within that class.

```rust
pub struct Capabilities(u32);

impl Capabilities {
    pub const NONE: Self = Self(0);
    pub const MMIO: Self = Self(1 << 0);
    pub const IRQ: Self = Self(1 << 1);
    pub const SPAWN_DRIVER: Self = Self(1 << 2);
}

pub struct ResourceGrants {
    pub mmio_regions: Vec<(u64, u64)>,
    pub irqs: Vec<u8>,
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

Capabilities belong to scheduler tasks. Because `TaskEntry` is copyable, it stores a raw pointer rather than a `Box<DriverCaps>`:

```rust
pub struct TaskEntry {
    // Register state, stack, FPU state, and other metadata...
    pub caps: *mut crate::process::capabilities::DriverCaps,
}
```

A normal task begins with a null pointer. `SpawnDriver` allocates the block on the heap, converts the `Box` with `Box::into_raw()`, and attaches the pointer to the new task. When the task is removed, `remove_task()` reconstructs exactly one `Box` and frees it:

```rust
if !entry.caps.is_null() {
    drop(unsafe { Box::from_raw(entry.caps) });
    entry.caps = core::ptr::null_mut();
}
```

This ownership protocol is safety-critical. `Box::from_raw()` may only be called once for an allocation. The pointer is therefore nulled immediately. Returning a mutable reference from `current_task_caps()` also depends on the kernel's current single-core execution model: no parallel context may create a second mutable reference to the same block.

---

## 5. The Syscall Boundary and ABI

The new numbers are appended to the existing ABI in [`kernel/src/syscall/types.rs`](../kernel/src/syscall/types.rs). Kernel and user space must agree on these exact numeric values:

```rust
pub enum SyscallId {
    // Existing syscalls 0..29
    MapPhysical = 30,
    UnmapPhysical = 31,
    IrqSubscribe = 32,
    IrqWait = 33,
    IrqAck = 34,
    SpawnDriver = 35,
    AllocDma = 36,
    FreeDma = 37,
    VirtToPhys = 38,
}
```

[`lib_driver/src/raw.rs`](../lib_driver/src/raw.rs) provides small assembly stubs for one, two, or three parameters. A three-argument call is implemented as follows:

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

The kernel's `dispatch_checked()` function in [`kernel/src/syscall/dispatch/mod.rs`](../kernel/src/syscall/dispatch/mod.rs) acts as a switchboard. For example, it routes `MapPhysical` as follows:

```rust
SyscallId::MAP_PHYSICAL =>
    driver::syscall_map_physical_impl(arg0, arg1 as usize, arg2),
```

The match only selects a handler. The actual validation and security decisions happen inside [`kernel/src/syscall/dispatch/driver.rs`](../kernel/src/syscall/dispatch/driver.rs).

`SpawnDriver` first verifies that the caller either holds `SPAWN_DRIVER` or is a privileged kernel task or task 1. It then validates and copies the file name from user memory and reads the optional `UserDriverGrants` request.

Next comes the step that makes the grant trustworthy: `driver_db::derive_grants()` resolves the binary name against the kernel's driver database, finds the matching device in the kernel's own PCI enumeration, and builds the `ResourceGrants` from that device's memory BARs and interrupt line. Nothing in the grant originates from user space. A binary that is not registered as a driver receives no grants at all, and a grant *request* for such a binary is refused outright — there is no device to validate it against. If the caller did supply a request, it is compared against the derived grant and rejected with `PermissionDenied` if it names a region or vector outside the bound device.

Only then does the syscall load the ELF image through the VFS, establish the parent relationship, and attach a newly allocated `DriverCaps` block to the new task. The requested capability bits are narrowed twice: unknown bits are discarded by `Capabilities::from_bits_truncate()`, and the result is masked to `driver_db::DRIVER_GRANTABLE_CAPS`. That mask omits `SPAWN_DRIVER`, so a driver can never inherit the authority to spawn further drivers with capabilities of its own choosing.

---

## 6. Mapping a Physical BAR into the Driver

Virtual memory gives each process a private view of addresses. Virtual address `0x1000` in two processes can refer to two unrelated physical frames. Page tables store this translation, and the CPU's `CR3` register identifies the root table of the current address space.

The branch reserves a dedicated virtual window for driver mappings:

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

## 7. Allocating Physically Contiguous DMA Memory

The Physical Memory Manager tracks free frames in bitmaps. A clear bit represents an available 4 KiB frame. The new `alloc_contiguous_frames()` method in [`kernel/src/memory/pmm/manager.rs`](../kernel/src/memory/pmm/manager.rs) scans each region and counts consecutive free bits. Its search can be understood schematically as follows:

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

## 8. Bridging an Interrupt to a Sleeping Process

The IRQ bridge in [`kernel/src/drivers/irq_bridge.rs`](../kernel/src/drivers/irq_bridge.rs) connects two very different execution contexts. A hardware interrupt must do as little work as possible: it cannot take blocking locks, allocate from the heap, or run a network stack. A normal process, however, may sleep and later execute complex Rust code.

There is one static `IrqBinding` for every PIC line:

```rust
pub struct IrqBinding {
    pub task_id: AtomicUsize,
    pub pending: AtomicBool,
    pub waitq: SingleWaitQueue,
}
```

`IrqSubscribe` claims a slot through `compare_exchange(0, task_id, ...)`, ensuring exclusive ownership:

```rust
if binding.task_id.compare_exchange(
    0,
    task_id,
    Ordering::AcqRel,
    Ordering::Acquire,
).is_err() {
    return Err(SyscallError::InvalidArg);
}
```

The kernel then registers a generic trampoline. When the interrupt arrives, this minimal top half only sets the pending flag and wakes the wait queue:

```rust
pub fn driver_irq_trampoline(
    vector: u8,
    regs: &mut SavedRegisters,
) -> *mut SavedRegisters {
    let idx = match irq_to_index(vector) {
        Some(index) => index,
        None => return regs as *mut SavedRegisters,
    };

    let binding = &IRQ_BINDINGS[idx];
    binding.pending.store(true, Ordering::Release);
    wake_all_single(&binding.waitq);
    regs as *mut SavedRegisters
}
```

The Release and Acquire orderings ensure that the awakened context observes the pending-state change. `IrqWait` first uses `swap(false, ...)` to consume an already pending event. An interrupt is therefore not lost merely because it arrived just before the syscall.

If no event is pending, the task registers with the wait queue and yields the CPU. After waking, it checks again. This loop matters because being awakened is not, by itself, proof that the expected interrupt is pending.

The ordinary interrupt dispatcher automatically sends an EOI for PIC interrupts. Driver-owned interrupts are excluded:

```rust
if is_pic_vector(vector)
    && is_in_service(vector - IRQ_BASE)
    && !irq_bridge::is_driver_irq(vector - IRQ_BASE)
{
    end_of_interrupt(vector - IRQ_BASE);
}
```

After `IrqWait`, the user driver reads the RTL8139 Interrupt Status Register. RTL8139 status bits are cleared by writing ones back to them. Only then does the process invoke `IrqAck`, allowing the kernel to send the PIC EOI:

```rust
irq::wait(self.irq, timeout_ms)?;
let status = self.mmio.read16(REG_ISR);
self.mmio.write16(REG_ISR, status);
irq::ack(self.irq)?;
```

This ordering avoids releasing the PIC line while the device still reports an active interrupt cause.

---

## 9. Initializing the RTL8139 Controller

After startup, the driver scans the PCI list again. The shell needed its scan only to check that the card is present; the driver needs its own scan to obtain the BAR and IRQ for actual operation. It locates vendor ID `0x10EC` and device ID `0x8139`, chooses a memory BAR, and calls `Mmio::map()`. That the driver reads these values itself is harmless: the kernel checks the mapping request against the grant it derived independently, so a driver that computed a wrong or malicious address gets `PermissionDenied` rather than the mapping.

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

The driver then reads six consecutive bytes from `REG_MAC0` to obtain the hardware MAC address. It allocates the RX and TX DMA regions described above and writes their physical addresses into the controller.

`CAPR` is the consumer pointer of the receive ring. The RTL8139 expects it to be offset backwards by 16 bytes, so the initial value is `0xFFF0`, which represents logical offset zero minus `0x10` with 16-bit wrapping.

The Receive Configuration Register is programmed as follows:

```rust
let rcr = RCR_AAP | RCR_APM | RCR_AM | RCR_AB | RCR_WRAP;
mmio.write32(REG_RCR, rcr);
```

This accepts all packets, packets matching the physical MAC, multicast, and broadcast. `RCR_WRAP` enables ring-buffer wraparound. Because `AAP` already accepts everything, the software network stack performs an additional destination-MAC check.

If the device has a valid IRQ and subscription succeeds, the driver enables receive and transmit success and error interrupts. Finally, it sets the receiver-enable and transmitter-enable bits in `CHIPCMD`.

---

## 10. Transmitting an Ethernet Frame

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

Sixty bytes is the minimum Ethernet-frame length without the Frame Check Sequence generated by hardware. Short protocol packets must therefore be padded.

The data path is intentionally efficient. Syscalls are needed to allocate and establish mappings, but each packet is written directly to mapped DMA memory and triggered with a direct MMIO write. There is no syscall for every register access.

---

## 11. Receiving an Ethernet Frame

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

Only packets with the Receive OK bit and a plausible length are accepted. Four CRC bytes are removed from the reported size, and the driver copies the frame into the caller's buffer.

The next record begins on a four-byte boundary after the device header and packet. When the offset reaches the end of the 8192-byte ring, it wraps modulo the ring size. Finally, the driver writes `rx_offset - 0x10` to `CAPR`, informing the device how much data software has consumed.

The driver's background event loop (`lib_driver_runtime::run_background_driver()`) polls the ring in a loop, feeding every received frame into the `lib_net` protocol stack and unconditionally forwarding it to any application waiting on this driver's channel:

```rust
while let Some(len) = device.poll_next_packet(&mut rx_buf) {
    let _event = stack.handle_rx_packet(&rx_buf[..len], |reply| {
        let _ = device.transmit(reply);
    });
    let _ = net_send(own_id, &rx_buf[..len]);
}
```

Although the IRQ infrastructure exists, this receive path does not call `wait_irq()`. It performs short spin loops between polling attempts.

What `stack.handle_rx_packet()` does with that byte slice — Ethernet framing, ARP resolution, IPv4, and ICMP Echo — belongs to the hardware-agnostic `lib_net` crate and is documented in full, byte layout included, in [`docs/networking.md`](networking.md).

---

## 12. Resource Lifetime and RAII

Rust's RAII model is useful for driver resources. A resource is tied to an object and released by its `Drop` implementation. `Mmio` unmaps its region when dropped, and `DmaBuffer` unmaps and frees its frames. `Rtl8139Device` directly owns all three objects. If the device value leaves a normal scope or is passed to `drop(device)`, cleanup occurs in reverse ownership order.

Before a regular `exit` or `quit`, the CLI calls `device.shutdown()`:

```rust
pub fn shutdown(&mut self) {
    self.mmio.write8(REG_CHIPCMD, 0x00);
    self.mmio.write16(REG_IMR, 0x0000);
}
```

The process then calls `process::exit()`, which has return type `!` and never returns to Rust. This detail is important: no stack unwinding occurs, so local variables in `_start()` are not normally dropped on this path. In the current program, the final cleanup is therefore not performed by the `Drop` handlers. Instead, the kernel destroys the task's complete user address space while reaping it. The generic VMM teardown releases PMM-managed leaf frames, including the DMA frames, through their reference counts. The scheduler separately frees the `DriverCaps` allocation.

The `Drop` implementations remain valuable if a mapping or DMA buffer leaves scope while the process continues, or if it is explicitly dropped before exit. A more precise regular shutdown should eventually call `drop(device)` before `process::exit()` rather than relying entirely on generic address-space destruction.

The ownership structure is:

```text
TaskEntry
  `-- DriverCaps
        |-- capability bits
        `-- MMIO and IRQ grants

Rtl8139Device
  |-- Mmio         --Drop on scope exit--> UnmapPhysical
  |-- RX DmaBuffer --Drop on scope exit--> FreeDma
  `-- TX DmaBuffer --Drop on scope exit--> FreeDma

Task reaping
  `-- complete user address space --> VMM teardown
```

MMIO and DMA have different ownership semantics. For MMIO, the device owns the physical address and the process owns only a virtual mapping. For DMA, the physical RAM frames were allocated for the process and must eventually return to the PMM.

---

## 13. Build Integration, ELF Layout, and QEMU

`lib_driver` and `rtl8139_user_program` are new Cargo workspace members. The driver is built for `x86_64-unknown-none` without the standard library. It uses `core`, the existing allocator through `alloc`, `lib_kaos`, and `lib_driver`.

The linker script [`drivers/rtl8139/link.ld`](../drivers/rtl8139/link.ld) selects `_start` as the entry point and places code at virtual address `0x0000_7000_0000_0000`. It creates two PT_LOAD segments. Code and read-only data occupy a readable and executable segment, while data and BSS occupy a readable and writable segment.

Page alignment between the segments is necessary because the kernel enforces ELF permissions at page granularity. If executable code and writable data shared one page, the loader could not represent the intended permissions cleanly.

The build helpers copy `RTL8139.BIN` into both BIOS and UEFI disk images. QEMU is also given an emulated RTL8139 device. On macOS, the scripts use `vmnet-bridged` with `en0`. On Linux, they expect a preconfigured TAP interface named `tap0`. The guest is therefore attached to a bridged Layer 2 network rather than only an isolated virtual network.

---

## 14. What the Tests Actually Verify

The new test programs validate the layers independently. [`capabilities_test.rs`](../kernel/tests/capabilities_test.rs) checks bit operations, the capability-free initial state of ordinary tasks, and attachment and cleanup of a `DriverCaps` block.

[`driver_mmio_test.rs`](../kernel/tests/driver_mmio_test.rs) simulates tasks with and without MMIO authority. It verifies rejection of missing capabilities, incorrect physical ranges, zero lengths, and overflowing addresses. The successful case checks bump-pointer advancement and unmapping.

[`driver_irq_test.rs`](../kernel/tests/driver_irq_test.rs) ensures that only the owner can subscribe to and acknowledge an IRQ line. A second task cannot claim the same line. The test invokes the trampoline directly, verifies the pending-event path, and acknowledges the event.

[`driver_spawn_test.rs`](../kernel/tests/driver_spawn_test.rs) checks missing `SPAWN_DRIVER` authority, invalid user pointers, and the exact 24-byte ABI layout of `UserDriverGrants`. It also covers the grant-derivation contract: that `SPAWN_DRIVER` is masked out of a spawned driver's capabilities, that the driver database resolves registered binaries case-insensitively and rejects unregistered ones, and that a grant request naming physical memory outside the bound device's BAR is refused.

[`driver_rtl8139_test.rs`](../kernel/tests/driver_rtl8139_test.rs) combines MMIO, IRQ, and DMA in a simulated RTL8139 task. It also imports the real network modules through `#[path]`, so the same parsers and serializers run in ordinary host unit tests and inside the QEMU kernel test environment.

The new [`test_all.sh`](../test_all.sh) runs user-space protocol tests, kernel tests under QEMU, `cargo fmt --check`, and Clippy, then produces a unified summary.

---

## 15. Security and Implementation Limits

This architecture isolates driver code from the kernel much better than a Ring 0 driver, but it is not yet equivalent to a production driver framework.

The most important hardware limitation is DMA without an IOMMU. The kernel gives the driver physical addresses, and a bus-master device can initiate physical memory transactions. Capabilities prevent the process from creating arbitrary CPU mappings, but they do not configure an IOMMU that limits the frames reachable by the device. A wrongly programmed or malicious DMA device could therefore access memory outside its intended buffers.

DMA bookkeeping is also minimal. `FreeDma` does not consult a per-task list of DMA allocations. It translates the supplied virtual pages, removes their mappings, and releases the resulting PFNs. A robust implementation should record the owner, base address, and length of every allocation and accept only exact matching frees. Otherwise, a privileged driver can supply an address or size that does not describe one of its own DMA allocations.

The MMIO bump allocator never reuses virtual holes. A long-lived process that repeatedly maps and unmaps regions still advances toward the stack guard. This simple design is adequate for the RTL8139's mostly one-time BAR and DMA setup.

The `flags` parameter of `MapPhysical` is documented as reserved but is not currently required to be zero. It has no effect today, but it should be validated before future semantics are assigned to those bits.

`IrqWait` accepts a timeout in milliseconds and passes it into the bridge, but the bridge currently ignores it. A nonzero timeout therefore does not time out. There is also no regular unsubscribe and task-cleanup path for IRQ bindings. If a subscribed driver exits, the static slot may remain bound. The test-only `reset_bindings_for_test()` function is not a substitute for production lifetime management.

The RTL8139 enables interrupts, but its interactive receive path only polls. The IRQ bridge is architecturally present and tested, yet it does not currently reduce CPU use in the running driver.

The regular `exit` command disables the device with `shutdown()` but does not call `drop(device)` before the non-returning `process::exit()`. A panic or `readline()` error skips even `shutdown()`. Although the kernel removes the process address space, a clean driver lifecycle should explicitly dismantle hardware state, IRQ binding, MMIO mapping, and DMA allocations in a defined order.

The `unload` command (§17) inherits the same limitation by construction, not by oversight: `DrvUnload` calls `scheduler::terminate_task()` directly, the same hard-kill path the scheduler already uses to reap a crashed or exited task. It does not run the driver's own shutdown code, so a still-DMA-active NIC is not told to disable bus-mastering before its address space and DMA frames are torn down. Combined with the lack of an IOMMU noted above, a driver unloaded while actively transferring data could in principle have the device write into memory that has since been reused for something else. Building a cooperative shutdown protocol (e.g. a dedicated syscall the driver polls for, mirrored by `unload` waiting for an acknowledgement before killing the task) is future work, not something this command attempts.

In the transmit path, the descriptor wait loop expires after 10,000 iterations without returning an error. The code still writes the slot and starts transmission. It also reports at least 60 bytes for short Ethernet frames without explicitly zeroing the additional bytes before every transmission. Data left from a previous slot use could therefore be emitted as Ethernet padding. A robust implementation should return `IoError` on timeout and clear the padding range.

The protocol stack the driver hands frames to has its own, separately documented limitations — no DHCP, no TCP/UDP, no IP fragmentation, ARP entries with no expiration timer, and so on; see [`docs/networking.md`](networking.md) §11.

Finally, QEMU networking depends on host configuration. `en0` is not the active interface on every Mac, and Linux requires `tap0` to be created with appropriate permissions and attached to a bridge before the scripts run.

---

## 16. The Entire Path as One Continuous Story

During boot, the kernel scans the PCI bus. It reads vendor IDs, device IDs, BARs, and IRQ lines and enables Memory Space and Bus Mastering. QEMU provides an emulated RTL8139 device.

When the user types `rtl8139` in the shell, the shell finds that device in the cached PCI list and passes the binary name together with the capability bits to `SpawnDriver`. The kernel validates the user pointers, looks the name up in its driver database, derives the MMIO and IRQ grants from its own view of that device's PCI configuration, loads `rtl8139.bin` as an ELF image into a separate address space, and attaches those grants to the new scheduler task.

The new process discovers the device again. It asks the kernel to map the granted BAR into its MMIO window. The kernel compares the complete physical range with the grant, creates user page-table entries with the required caching and execution attributes, and returns a virtual address. The process can now use volatile loads and stores to access its device registers directly, but it cannot map unrelated physical regions.

For reception and transmission, the driver requests contiguous DMA frames. The kernel marks them in the PMM, maps them into the same process, and returns both virtual and physical base addresses. The driver writes the physical addresses into RTL8139 registers. CPU and network card can now refer to the same memory through their respective address views.

During transmission, the hardware-agnostic `lib_net` protocol stack (`docs/networking.md`) serializes a complete Ethernet/IPv4/ICMP frame. The driver copies that completed frame into a TX DMA slot and starts the hardware by writing a TSD register. During reception, the card writes a frame into the RX ring. The driver reads the device-specific ring header, removes the CRC, and passes the remaining byte slice to `lib_net`, which may respond automatically with an ARP Reply or an ICMP Echo Reply.

When the user exits through `exit` or `quit`, the receiver and interrupt mask are disabled. `process::exit()` then terminates the task without Rust unwinding. The kernel destroys the complete user address space and releases PMM-managed DMA frames; the scheduler also frees the capability block. The waiting shell resumes. The existing `Drop` implementations would perform targeted MMIO and DMA cleanup, but they are not executed on this exact path without an explicit `drop(device)`.

The branch therefore demonstrates a complete vertical slice through an operating system: PCI configuration, page tables, process permissions, syscalls, DMA ring buffers, and network protocols. Because the RTL8139 driver runs in Ring 3, the boundaries between hardware, kernel mechanism, user-space abstraction, and protocol logic become especially clear.

---

## 17. The `drivers.bin` Management Application

Loading a driver used to be a single shell command (`load <name.drv>`) with no counterpart for unloading one or seeing what was currently running. `drivers.bin` (built from [`user_programs/drivers`](../user_programs/drivers)) replaces that one command with a small, standalone Ring-3 REPL dedicated to driver lifecycle management, structurally identical to the shell's own read-eval-print loop: a prompt, a line read via `console::readline()`, and a match over the first whitespace-separated word.

It understands four commands. `help` prints the command list. `list` calls `DrvListCount` and then `DrvListEntry` for each index, printing every currently registered driver's name and packed task id (or a note that none are loaded). `load <name.drv>` runs the same PCI-resolution-then-`SpawnDriver` logic the shell used to run directly (§3-§4), now living in [`user_programs/drivers/src/load_driver.rs`](../user_programs/drivers/src/load_driver.rs). `unload <name>` calls the new `DrvUnload` syscall (§15 documents its hard-kill semantics).

An illustrative session, after typing `drivers.bin` at the shell prompt to launch it (derived directly from `execute_command`'s match arms — see the verification note below for what was actually observed running):

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
drivers> unload nic:rtl8139
[drivers] Driver 'nic:rtl8139' unloaded.
drivers> list
No drivers loaded.
```

`drivers.bin` has no dedicated `exit` command of its own yet (unlike the shell) — leaving it today means power-cycling or, in a future revision, adding an explicit `exit`/`quit` command mirroring the shell's.

`load`/`unload`'s command-line parsing (present vs. missing argument, trailing words ignored, case-sensitive dispatch) is factored into a pure `parse_command()` function and unit-tested on the host without touching a syscall — the same pattern §14 describes for `resolve_driver_filename`. The syscall-touching behavior of `list`/`load`/`unload` themselves is covered by the kernel-side tests referenced in §14 and §18.

**Verification note:** this environment has no hardware-accelerated x86_64 virtualization (the build host is `aarch64`), so booting the real BIOS/bootloader chain under QEMU's software CPU emulation is slow, and driving the guest's PS/2-keyboard-based REPL interactively would require scripting raw scancode injection via QEMU's QMP `input-send-event` — judged disproportionate effort for this check. What was verified directly: the full disk image (kernel with the new `DrvUnload`/`DrvListCount`/`DrvListEntry` syscalls and `Exec` capability delegation, the updated `SHELL.BIN`, and the new `DRIVERS.BIN`) builds end to end via `helper_build_user_programs.sh` and `helper_make_fat32_bios_image.sh`; `DRIVERS.BIN` is present in the resulting FAT32 image alongside the other user programs; and the image boots successfully through the full bootsector → 16-bit loader → 64-bit loader → kernel chain to a working, keyboard-ready `SHELL.BIN` prompt with no panic, matching every prior boot (confirmed via a captured serial log). The transcript above reflects `drivers.bin`'s actual command-dispatch code, not a captured keystroke session — a human with local keyboard/display access should run through it once to confirm before relying on it in production.

---

## 18. Capability Delegation via `Exec`

`load` and `unload` both require capabilities (`SPAWN_DRIVER`, `UNLOAD_DRIVER`) that an ordinary Ring-3 program does not have. Before this feature, that was not a problem: `load` ran *inside* the shell's own process, and the shell is the one task marked privileged at boot (`kernel/src/main.rs`), so it sailed through `SpawnDriver`'s authorization check directly. Moving `load`/`unload` into a separate `drivers.bin` process broke that shortcut — a process started via `Exec` is, and always was, unconditionally unprivileged (`process::exec_from_vfs`'s own doc comment), regardless of who started it.

The fix is a narrow delegation mechanism on `Exec` itself, not a way to make `drivers.bin` privileged. `Exec` gained a second argument, `requested_caps`, and the kernel computes what to actually grant with a small, pure, unit-tested function:

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

A privileged caller (the shell) may delegate anything it asks for. An unprivileged caller may delegate at most the capabilities it already holds itself — the intersection of what it has and what it asks for — so a compromised or buggy unprivileged process can never manufacture a capability out of thin air and hand it to a child. When capabilities are granted, `syscall_exec_impl` attaches a `DriverCaps` block to the new task exactly the way `SpawnDriver` does for a driver it spawns (§5), just with `ResourceGrants::default()` — an empty MMIO/IRQ grant, since `drivers.bin` itself never touches hardware directly.

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

Every other program the shell execs — `TUI.BIN`, `KBASIC.BIN`, a plain `hello.bin` — still gets exactly zero delegated capabilities, identical to `Exec`'s behavior before this feature existed. This split mirrors `SpawnDriver`'s own design: the kernel enforces a hard security *invariant* (a caller can never delegate more than it has), while a trusted, auditable piece of user-space code owns the *policy* of who the invariant actually applies to. Both the mechanism (`resolve_delegated_capabilities`) and the policy (`requested_capabilities_for`) are covered by their own host-level unit tests, and the full path — shell delegates, `drivers.bin` receives exactly those three capabilities and nothing more, and every other exec'd program still receives none — is covered end-to-end by a dedicated kernel integration test suite (`kernel/tests/exec_capability_delegation_test.rs`).

