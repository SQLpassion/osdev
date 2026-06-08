# KAOS Rust Kernel: High-Precision Timekeeping & Timer Driver

This document explains the high-precision timer driver, the underlying hardware components (PIT and TSC), the hybrid timekeeping design, and the user-mode syscall interface in the KAOS Rust Kernel.

---

## 1. Hardware Architecture (The Real Timers)

The KAOS kernel utilizes two classic x86 hardware structures to achieve both periodic interrupts and high-precision, low-overhead time queries.

```
       +--------------------------------------------+
       |           BiosInformationBlock             |
       |  (Populated by bootloader from RTC CMOS)   |
       +---------------------+----------------------+
                             |
                             v Initial Boot Time
+----------------------------+----------------------+
|                     TimeManager                    |
|  Exposes current time via TSC offset scaling      |
+---------------------+----------------------+------+
                      ^                      ^
                      | TSC Calibration      | TSC query (rdtsc)
+---------------------+----------------------+------+
|    PIT Channel 2    |           CPU TSC           |
| (Intel 8253/8254)   | (Time Stamp Counter) |
+---------------------+----------------------+------+
```

### 1.1 Intel 8253/8254 Programmable Interval Timer (PIT)
The PIT is a microchip that contains three independent 16-bit counter channels. It is driven by an internal oscillator operating at a base frequency of **1,193,182 Hz** (approximately 1.193 MHz).

*   **Channel 0 (Port `0x40`):** Mapped to IRQ0. It generates periodic interrupts to drive preemptive scheduling (configured to run at 250 Hz in `KernelMain`).
*   **Channel 2 (Port `0x42`):** Historically connected to the PC speaker. Its gate can be enabled/disabled via the System Control Port B (`0x61`). In KAOS, it is repurposed for boot-time calibration.
*   **Command Register (Port `0x43`):** Configures operating modes, read/write policies, and latching commands.

### 1.2 CPU Time Stamp Counter (TSC)
The TSC is a 64-bit register present on all modern x86/x86_64 processors. It increments on every CPU clock cycle.
*   **Instruction:** Read via the `rdtsc` or `rdtscp` assembly instructions.
*   **Precision:** Nanosecond-level resolution depending on the CPU core frequency.
*   **Pros/Cons:** Querying the TSC takes only a few CPU cycles (zero I/O overhead) but its raw frequency is not fixed across different hardware and must be calibrated.

---

## 2. Hybrid Timekeeping Design

To avoid drifting software clocks and prevent slow motherboard I/O-port CMOS operations during runtime, the kernel implements a hybrid bootstrapping + calibration approach.

### 2.1 Phase 1: Bootstrapping the Start Time
During early system start, the 16-bit bootloader reads the hardware Real-Time Clock (RTC) from the CMOS non-volatile memory and populates the **BIOS Information Block (BIB)** at the physical memory address `0x1000` (`BIB_OFFSET`).
Upon initialization, the kernel reads this block once to establish the base boot time ($T_{\text{start}}$):

$$T_{\text{start}} = \text{BIB.year} \text{ / } \text{BIB.month} \text{ / } \text{BIB.day} \text{ } \text{BIB.hour}:\text{BIB.minute}:\text{BIB.second}$$

### 2.2 Phase 2: TSC Calibration
To scale the raw TSC cycles into real-world microseconds, the kernel calibrates the TSC against PIT Channel 2 at boot:
1.  **Preparation:** Disable the PIT Channel 2 gate (Port `0x61` bit 0 = 0).
2.  **Mode Select:** Program PIT2 to Mode 0 (Interrupt on Terminal Count) via Port `0x43` (Command `0xB0`).
3.  **Count Setup:** Set the countdown divisor to `11,931` (LOBYTE `0x9B`, HIBYTE `0x2E`), representing exactly a **10-millisecond delay** ($\frac{11931}{1193182 \text{ Hz}} \approx 0.010 \text{ s}$).
4.  **Measurement:** Enable the gate (Port `0x61` bit 0 = 1), capture the start TSC, poll the PIT counter using latch commands (`0x80`) until it reaches zero, and capture the end TSC.
5.  **Frequency Calculation:** 
    $$\text{cycles\_per\_us} = \frac{\text{TSC}_{\text{end}} - \text{TSC}_{\text{start}}}{10,000}$$

### 2.3 Phase 3: Runtime Extrapolation
A global static `TimeManager` protected by a `SpinLock` stores the bootstrapping baseline. When a thread queries the system time, the current value is extrapolated dynamically:

$$\text{Current Time} = T_{\text{start}} + \text{add\_seconds}\left( \frac{\text{Current TSC} - \text{Boot TSC}}{\text{cycles\_per\_us} \times 1,000,000} \right)$$

---

## 3. Module Layout & Implementation

The time driver is fully implemented in Rust under `src/drivers/time/`:

*   **[mod.rs](../src/drivers/time/mod.rs)**: Re-exports the public API (`init`, `get_time`, `DateTime`, `rdtsc`) and keeps imports clean.
*   **[types.rs](../src/drivers/time/types.rs)**: Holds the `DateTime` calendar struct and implements rollover calculations (`add_seconds`, leap year checks, and month day limits).
*   **[calibration.rs](../src/drivers/time/calibration.rs)**: Houses the assembly `rdtsc` wrapper and the PIT2 10ms-delay sweep loop.
*   **[manager.rs](../src/drivers/time/manager.rs)**: Contains the `TimeManager` logic and handles safe structure deserialization from the physical `BIB_OFFSET`.

---

## 4. Syscall Interface (Ring 3 Access)

The kernel exposes the high-precision system time to Ring-3 applications through an interrupt gate.

### 4.1 GetTime Syscall (ID 27 / `GET_TIME`)
*   **Handler:** `syscall_get_time_impl(out_ptr: *mut UserDateTime)`
*   **Safety checks:** The kernel validates that the output buffer pointer lies entirely within user canonical space (`is_valid_user_buffer`) and is writable.
*   **User Struct:** `UserDateTime` is `#[repr(C)]` and aligned to 16 bytes using a 7-byte trailing padding array:
    ```rust
    #[repr(C)]
    pub struct UserDateTime {
        pub year: i32,
        pub month: u8,
        pub day: u8,
        pub hour: u8,
        pub minute: u8,
        pub second: u8,
        pub _padding: [u8; 7],
    }
    ```

### 4.2 PollKey Syscall (ID 28 / `POLL_KEY`)
To allow interactive applications (such as the TUI monitor) to update their clocks in real-time, the kernel also exposes a non-blocking keyboard polling syscall.
*   **Handler:** `syscall_poll_key_impl() -> SyscallResult<u64>`
*   **Behavior:** Instead of blocking the active thread on `INPUT_WAITQUEUE` like `sys_read_key`, it queries the keyboard ring buffer directly. It returns the encoded key code if available, or `0` if empty, allowing the caller to yield via `sys_yield` and redraw the frame periodically.
