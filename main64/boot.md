# KAOS Multi-Stage Boot Process

This document describes the detailed mechanics and architecture of the multi-stage boot process of the KAOS operating system.

---

## 1. Overview and Flowchart

The KAOS boot process is divided into three successive stages. Each stage has a specific task aligned with the hardware state of the x86 processor.

```
+------------------------------------------------------------+
|                  BIOS POST (Power-On Self-Test)            |
+------------------------------------------------------------+
                              |
                              v (Loads MBR/bootsector to 0x7C00)
+------------------------------------------------------------+
| STAGE 1: Bootsector (bootsector.asm)                       |
| - Mode: 16-Bit Real Mode                                   |
| - Loads KLDR16.BIN (Stage 2) to 0x2000                     |
| - Loads KLDR64.BIN (Stage 3) to 0x3000                     |
| - Uses: BIOS INT 0x13 (Disk Read)                          |
+------------------------------------------------------------+
                              |
                              v (Jumps to 0x2000)
+------------------------------------------------------------+
| STAGE 2: 16-Bit Loader (kaosldr_16 / longmode.asm)         |
| - Mode: 16-Bit Real Mode -> 64-Bit Long Mode               |
| - Queries BIOS info (E820 Memory Map, system time)         |
| - Enables A20 Gate                                         |
| - Builds 4-level page tables at 0x9000                     |
| - Enables Paging/PAE and switches CPU to Long Mode         |
+------------------------------------------------------------+
                              |
                              v (64-Bit jump to 0x3000)
+------------------------------------------------------------+
| STAGE 3: 64-Bit Loader (kaosldr_64_rust / Rust Loader)     |
| - Mode: 64-Bit Long Mode (Ring 0)                          |
| - Initializes stack at 0x400000                            |
| - Reads FAT12 filesystem via direct ATA PIO ports          |
| - Loads KERNEL.BIN to 0x100000 (1 MB mark)                 |
| - Jumps to Kernel Entry                                    |
+------------------------------------------------------------+
                              |
                              v
+------------------------------------------------------------+
|                  KAOS Rust Kernel (kernel_rust)            |
+------------------------------------------------------------+
```

---

## 2. Memory Map During Boot

During the different boot stages, the memory layout in RAM changes continuously.

### Physical Memory Allocation After Stage 2 Completion

```
Address           Size        Description
+--------------+ ----------- +------------------------------------------+
| 0x00000000   | 1 KB        | Real Mode Interrupt Vector Table (IVT)   |
| 0x00000400   | 256 Bytes   | BIOS Data Area (BDA)                     |
| 0x00000500   | 2.75 KB     | Buffer for FAT12 Root & FAT (Stage 1)    |
| 0x00001000   | 512 Bytes   | BIOS Information Block (BIB)             |
| 0x00001200   | 3.5 KB      | E820 Memory Map buffer                   |
| 0x00002000   | 4 KB        | KLDR16.BIN (Stage 2 Code & GDT)          |
| 0x00003000   | Variable    | KLDR64.BIN (Stage 3 Code / Rust Loader)  |
| 0x00007C00   | 512 Bytes   | Bootsector (Stage 1)                     |
| 0x00009000   | 4 KB        | Page Map Level 4 (PML4)                  |
| 0x0000A000   | 4 KB        | PDPT (Identity Mapping)                  |
| 0x0000B000   | 4 KB        | PD (Identity Mapping)                    |
| 0x0000C000   | 4 KB        | PDPT (Higher Half Mapping)               |
| 0x0000D000   | 4 KB        | PD (Higher Half Mapping)                 |
| 0x0000E000   | 32 KB       | Page Tables 0-7 (16 MB mapped)           |
| 0x00100000   | Variable    | KERNEL.BIN (Loaded by Stage 3)           |
| ...          |             | ...                                      |
| 0x00400000   | 2 MB        | Temporary Stack for Stage 3 (RSP)        |
+--------------+ ----------- +------------------------------------------+
```

---

## 3. Technical Stage Details

### Stage 1: Bootsector (`bootsector.asm`)
* **Initial State:** After BIOS POST, the CPU is in 16-bit Real Mode. The bootsector is loaded at address `0x7C00`.
* **Processor State:** CS:IP = `0x0000:0x7C00`, registers are undefined, interrupts are enabled.
* **Flow:**
  1. **Stack Initialization:** Sets up a temporary stack at `SS=0x7000`, `SP=0x8000`.
  2. **Filesystem Parsing (FAT12):** Since the bootsector is formatted in FAT12, it parses the Root Directory Table to locate the two loader stages.
  3. **Loading:**
     * Reads `KLDR64.BIN` via BIOS interrupt `INT 0x13, AH=0x02` into memory at address `0x3000` (`KAOSLDR64_OFFSET`).
     * Reads `KLDR16.BIN` into memory at address `0x2000` (`KAOSLDR16_OFFSET`).
  4. **Handoff:** Performs a `FAR JUMP` (or `CALL`) to address `0x2000` to execute Stage 2.

### Stage 2: 16-Bit Loader (`kaosldr_16`)
* **Initial State:** Running at address `0x2000` in 16-bit Real Mode.
* **Flow:**
  1. **Gather BIOS Data:** 
     * Reads the current date and time from the BIOS (`INT 0x1A`).
     * Retrieves the physical memory map (E820 table) via BIOS interrupt `INT 0x15, AX=0xE820` and saves it at `0x1200`. This information is later passed to the kernel.
  2. **Enable A20 Gate:** Unlocks the 21st address line of the CPU via Keyboard Controller Port `0x92` or BIOS services to allow memory access above 1 MB.
  3. **Set Up Page Tables (Paging):**
     * Initializes a 4-level paging structure starting at address `0x9000` (PML4).
     * Builds an **Identity Mapping** for the first 16 MB (Virtual Address `0x00000000` $\rightarrow$ Physical Address `0x00000000`).
     * Builds a **Higher Half Mapping** for the kernel (Virtual Address `0xFFFF800000000000` $\rightarrow$ Physical Address `0x00000000`).
  4. **Mode Switch:**
     * Disables all hardware interrupts (CLI) and loads a zero-length IDT (`LIDT`).
     * Enables PAE (Physical Address Extension) and PGE bits in `CR4`.
     * Loads the PML4 register into `CR3` (`0x9000`).
     * Enables Long Mode in the EFER (Extended Feature Enable Register) MSR via `wrmsr` (setting LME and NXE bits).
     * Enables Paging and Protected Mode by setting the respective bits in `CR0`.
     * Loads the Global Descriptor Table (`GDT`) containing the 64-bit code and data segment descriptors.
  5. **Switch to 64-Bit Mode:**
     * Executes an intersegment jump (`JMP CODE_SEG:LongMode`). This officially transitions the CPU into **64-bit Long Mode** (Submode: *64-bit mode*).
     * Sets segment registers `DS`, `ES`, `FS`, `GS`, `SS` to the 64-bit data selector.
     * Sets the 64-bit stack pointer `RSP` to `0x400000`.
     * Jumps via `JMP 0x3000` into the loaded `KLDR64.BIN`.

### Stage 3: 64-Bit Loader (`kaosldr_64_rust`)
* **Initial State:** Starts in 64-bit Long Mode at address `0x3000`. The code is written in Rust (`no_std`, `no_main`).
* **Flow:**
  1. **Stack & VGA Initialization:** Establishes stack access and initializes the VGA text writer class for error printing.
  2. **ATA PIO Driver:** Since BIOS interrupts are no longer accessible in Long Mode, the loader accesses the disk controller directly using I/O port commands (`in` / `out` on ports `0x1F0`–`0x1F7`).
  3. **File Search & Load:**
     * Parses the FAT12 structure on the boot drive.
     * Searches for the kernel (`KERNEL  BIN`).
     * Reads the kernel sectors chunk-by-chunk via the PIO data port `0x1F0` directly into the physical destination address `0x100000` (1 MB mark).
  4. **Execute Kernel:**
     * Jumps to the entry function of the loaded kernel (at address `0x100000`).
     * The bootloader's execution is now finished, and the kernel assumes full control.

---

## 4. Design Advantages

1. **No Real-Mode Memory Hacks:** The kernel is loaded only after entering 64-bit Long Mode. This avoids dealing with "Unreal Mode" or paging memory fragments across segment boundaries in Real Mode.
2. **Compact Filesystem Parsing in Rust:** The complex logic for FAT12 filesystem parsing and the ATA disk driver is written entirely in safe, modern Rust. Only the minimal necessary CPU transition code remains in assembly.
3. **Preservation of BIOS Data:** Critical system statistics (such as the E820 Memory Map), which can only be queried through 16-bit BIOS interrupts, are securely saved in RAM before the mode switch so they remain accessible to the kernel.
