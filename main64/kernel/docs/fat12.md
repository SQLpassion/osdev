# FAT12 File System: Unified Architectural Design & Usage Specification

This document provides an exhaustive technical specification of the FAT12 file system implementation in the KAOS kernel. It covers the physical format parameters of the media, the low-level Ring 0 kernel-space driver engine (defined in `src/io/fat12.rs`), the system call dispatch layer, and the safe Ring 3 user-space system call interfaces (defined in `common/syscall.rs`).

---

## 1) Introduction & Architectural Goals

The KAOS kernel's FAT12 implementation is designed around three primary principles:
1. **Safety and Robustness**: Memory operations in unprivileged Ring 3 are completely decoupled from raw pointers. The kernel enforces pointer and bounds checks on all system call arguments to protect kernel memory.
2. **Decoupling of Concerns**: The physical disk reading layer, the raw memory parser, the filesystem metadata traverser, and the visual console presenter are cleanly isolated.
3. **Deterministic Cache-Free State**: To simplify concurrency and eliminate the risk of stale metadata, the driver operates directly on the raw sectors of the floppy disk. All operations (read, write, delete) read and flush changes to disk immediately.

---

## 2) Volume Geometry & FAT12 Physical/Logical Layout

The file system operates on a standard 1.44 MiB 3.5" floppy disk profile. The layout assumes a sector size of 512 bytes, one reserved boot sector, two copies of the File Allocation Table (FAT), and nine sectors per FAT.

### 2.1) Media Partitioning
The volume is organized into contiguous logical blocks (LBAs):

```text
+---------------------------------------------------------------------------------+
| LBA Range    | Region Name            | Description                             |
+--------------+------------------------+-----------------------------------------+
| LBA 0        | Reserved Sector        | Contains the bootloader bootsector.     |
| LBA 1..9     | File Allocation Table 1| Primary FAT used for cluster linkage.   |
| LBA 10..18   | File Allocation Table 2| Mirror copy of FAT 2 (unused by engine).|
| LBA 19..32   | Root Directory Region  | Fixed area containing 224 file entries. |
| LBA 33..2879 | Data Clusters Area     | Storage for file content (clusters 2+). |
+---------------------------------------------------------------------------------+
```

Visualizing this geometry on disk yields the following sequential map:

```text
FAT12 Volume Layout (1.44 MiB floppy profile)

LBA 0                      LBA 1                 LBA 10                LBA 19                LBA 33
+--------------------------+---------------------+---------------------+---------------------+---------------------+
| Reserved Sector (Boot)   | FAT #1 (9 sectors)  | FAT #2 (9 sectors)  | Root Dir (14 sec)   | Data Clusters       |
| count = 1                | LBA 1..9            | LBA 10..18          | LBA 19..32          | LBA 33..2879        |
+--------------------------+---------------------+---------------------+---------------------+---------------------+
```

### 2.2) Memory Offset Calculations
The geometry bounds are enforced in the driver via static constants:
* **`BYTES_PER_SECTOR`**: 512 bytes.
* **`FAT_COUNT`**: 2 tables.
* **`SECTORS_PER_FAT`**: 9 sectors.
* **`RESERVED_SECTORS`**: 1 sector.
* **`ROOT_DIRECTORY_ENTRIES`**: 224 slots.

From these layout parameters, the following structural boundary markers are computed:
* **Root Directory LBA**: Calculated as $\text{Reserved Sectors} + (\text{FAT Count} \times \text{Sectors per FAT}) = 1 + (2 \times 9) = 19$.
* **Root Directory Sectors**: Calculated as $(\text{Root Directory Entries} \times 32 \text{ bytes}) / \text{Sector Size} = (224 \times 32) / 512 = 14$ sectors.
* **Data Area Start LBA**: Calculated as $\text{Root LBA} + \text{Root Sectors} - 2 = 19 + 14 - 2 = 31$.
* **Cluster Translation LBA**: For any given cluster $C$ (where $C \ge 2$), its physical starting sector is calculated as:
  $$\text{LBA}(C) = (C - 2) \times \text{Sectors per Cluster} + \text{Data Area Start LBA} = C + 31$$

---

## 3) Raw Directory Entry Structure

The root directory region is a flat table of 224 entries, each exactly 32 bytes wide. Rather than mapping memory directly to Rust packed structs (which can introduce alignment issues or undefined behavior during raw byte reinterpretation), the driver parses the bytes using explicit offsets.

### 3.1) Byte Fields
```text
32-Byte Short Directory Entry Layout:

Offset (Hex)  Size (Bytes)  Field Description
+------------+-------------+-------------------------------------------------+
| 0x00       | 8           | Filename (Base name, padded with spaces)        |
| 0x08       | 3           | Extension (Padded with spaces)                  |
| 0x0B       | 1           | Attribute Flags                                 |
| 0x0C       | 1           | NT Reserved / Case Flags (unused)               |
| 0x0D       | 1           | Creation Time tenths (unused)                   |
| 0x0E       | 2           | Creation Time (unused)                          |
| 0x10       | 2           | Creation Date (unused)                          |
| 0x12       | 2           | Last Access Date (unused)                       |
| 0x14       | 2           | High Word of First Cluster (0 in FAT12)         |
| 0x16       | 2           | Last Write Time (unused)                        |
| 0x18       | 2           | Last Write Date (unused)                        |
| 0x1A       | 2           | Starting Cluster (Low Word, little-endian)      |
| 0x1C       | 4           | File Size (Bytes, little-endian u32)            |
+------------+-------------+-------------------------------------------------+
```

### 3.2) Attribute Byte Encoding
The attribute byte at offset `0x0B` is parsed as a bitmask:

```text
 Bit:    7      6       5       4       3        2       1       0
      +------+------+------+------+--------+------+------+------+
      | Res  | Res  | ARCH | DIR  | VOLUME | SYS  | HID  | RO   |
      +------+------+------+------+--------+------+------+------+
```
* **`RO` (0x01)**: Read Only.
* **`HID` (0x02)**: Hidden File.
* **`SYS` (0x04)**: System File.
* **`VOLUME` (0x08)**: Volume Label.
* **`DIR` (0x10)**: Directory Indicator.
* **`ARCH` (0x20)**: Archive Indicator.
* **`LFN` (0x0F)**: Long File Name helper entry (represented by the combination of Read Only, Hidden, System, and Volume Label flags).

---

## 4) Directory Parsing & Classification Engine

The parsing engine decodes raw sectors into logical records without allocating heap memory, relying instead on a callback mechanism.

```text
Root Directory Buffer
[ 32-Byte Slot 0 ] ---> State Check: 0x00 (End) -> Terminate Loop
[ 32-Byte Slot 1 ] ---> State Check: 0xE5 (Deleted) -> Skip Slot
[ 32-Byte Slot 2 ] ---> State Check: Attribute 0x0F (LFN) -> Skip Slot
[ 32-Byte Slot 3 ] ---> State Check: Attribute 0x08 (Volume) -> Skip Slot
[ 32-Byte Slot 4 ] ---> State Check: Active File -> Parse Name & Metadata -> Trigger Callback
```

### 4.1) Entry State Classification
Each 32-byte slot is classified into one of three parser states based on its first byte and attributes:
1. **End**: The first byte of the filename is `0x00`. This indicates that there are no further entries in the root directory. Scanning terminates immediately.
2. **Skip**: The slot is not active:
   * First byte is `0xE5` (indicating the file was deleted).
   * Attribute byte at `0x0B` is `0x0F` (LFN helper entry).
   * Attribute byte contains the `VOLUME` flag (`0x08`).
3. **Active**: The slot represents a listable short-name file or directory.

### 4.2) Short-Name Normalization
The raw 11-character name field (8 characters base + 3 characters extension) is converted into a standard lowercase name for display:
1. The base name (bytes 0 to 7) is read, and any trailing spaces are stripped.
2. The extension (bytes 8 to 10) is read, and any trailing spaces are stripped.
3. If the extension is not empty, a dot (`.`) is inserted between the base and the extension.
4. All characters are converted to lowercase ASCII.
* *Example*: On-disk bytes `[54 45 53 54 20 20 20 20 | 54 58 54]` ("TEST    TXT") are translated to the string `"test.txt"`.

---

## 5) FAT12 Bitpacking & Navigation Mathematics

FAT12 allocation entries are 12 bits wide. This means two entries are packed into three bytes on disk. Given consecutive bytes $B_0$, $B_1$, and $B_2$:
* The first entry ($E_{even}$) occupies the lower 8 bits of $B_0$ and the lower 4 bits of $B_1$.
* The second entry ($E_{odd}$) occupies the upper 4 bits of $B_1$ and the 8 bits of $B_2$.

```text
Byte Layout:
         Byte 0                     Byte 1                     Byte 2
   +------------------+       +------------------+       +------------------+
   | 7              0 |       | 7   4 | 3      0 |       | 7              0 |
   +------------------+       +-------+----------+       +------------------+
     [  Low 8 of E0  ]         [Hi 4 E1|Lo 4 E0]           [  High 8 of E1  ]
```

### 5.1) Decoding a FAT Entry
To read the next cluster link for a given cluster $C$:
1. Calculate the byte offset in the FAT sector buffer:
   $$\text{offset} = C + (C / 2)$$
2. Read the 16-bit word at this offset (little-endian).
3. Apply the parity rules:
   * **If $C$ is even**:
     $$\text{next\_cluster} = \text{word} \land \text{0x0FFF}$$
   * **If $C$ is odd**:
     $$\text{next\_cluster} = \text{word} \gg 4$$

### 5.2) Encoding a FAT Entry
To write a new link value $V$ to cluster $C$:
1. Calculate the byte offset: $\text{offset} = C + (C / 2)$.
2. Read the existing bytes at $\text{offset}$ and $\text{offset} + 1$.
3. Apply the parity rules:
   * **If $C$ is even**:
     $$\text{byte}_0 = V \land \text{0xFF}$$
     $$\text{byte}_1 = (\text{existing\_byte}_1 \land \text{0xF0}) \lor ((V \gg 8) \land \text{0x0F})$$
   * **If $C$ is odd**:
     $$\text{byte}_0 = (\text{existing\_byte}_0 \land \text{0x0F}) \lor ((V \ll 4) \land \text{0xF0})$$
     $$\text{byte}_1 = (V \gg 4) \text{ as u8}$$
4. Write the modified bytes back to the FAT sector buffer and flush them to the disk.

---

## 6) Control Flow Implementations (Ring 0)

### 6.1) Reading a File (`read_file`)
1. **Name Normalization**: The target filename (e.g. `"test.txt"`) is validated and converted to the uppercase space-padded 8.3 representation (e.g. `"TEST    TXT"`).
2. **Directory Lookup**: The 14 root directory sectors are scanned to find the matching entry. If missing, `Fat12Error::NotFound` is returned. If it is a directory, `Fat12Error::IsDirectory` is returned.
3. **Chain Traversal**: The FAT is loaded into memory. Starting at the entry's starting cluster, the driver loops:
   * Translates the cluster ID to an LBA: $\text{LBA} = C + 31$.
   * Reads the sector data.
   * Copies the bytes to the output buffer, stopping when the total bytes copied matches the file size.
   * Resolves the next cluster via `fat12_next_cluster`.
4. **Safety Checks**: The loop aborts with `Fat12Error::CorruptFatChain` if:
   * A cluster value is less than 2.
   * A cluster indicates a reserved or bad cluster range ($[0xFF0, 0xFF7]$).
   * A circular loop is detected (checked using a tracking bitmap).

### 6.2) Allocating a Cluster (`allocate_new_cluster`)
1. **FAT Scan**: The FAT sector buffer is searched cluster-by-cluster starting at index 2.
2. **Find Empty Slot**: The first entry containing `0x000` is selected.
3. **Write Link**: 
   * The new cluster is marked as EOF (`0xFFF`).
   * If a previous cluster is provided, its FAT entry is updated to point to the new cluster.
4. **Flush & Zero**: The modified FAT sectors are written to disk. The disk sector corresponding to the new cluster ($\text{LBA} = C + 31$) is zeroed out to prevent read contamination.

### 6.3) Writing a File (`write_file_fd`)
1. **FD Table Locking**: The global descriptor table is locked. The write offset determines how many sectors into the cluster chain the write pointer is.
2. **Chain Traversal**: The cluster chain is followed. If the write offset exceeds the existing chain length, new clusters are allocated.
3. **Data Write**:
   * For each block, if the write size is less than 512 bytes, the current sector is read into a temporary buffer first.
   * The new data is copied into the buffer at the write offset, and the sector is written back to disk.
4. **Directory Synchronization**: The directory entry is updated with the new file size and starting cluster, and written back to disk.

### 6.4) Deleting a File (`delete_file`)
1. **Directory Mark**: The file entry is located in the root directory. Its first character is overwritten with the deleted flag `0xE5`, and the sector is written to disk.
2. **Chain Cleanup**: The FAT is loaded. Starting at the file's start cluster, the driver:
   * Resolves the next cluster.
   * Clears the current cluster's FAT entry to `0x000`.
   * Fills the corresponding sector on disk with zeros.
3. **Flush**: The updated FAT is written back to the disk.

### 6.5) Concurrency & File Descriptor Table
An active file descriptor table is maintained in the kernel. Access to this table is guarded by a kernel `SpinLock` to ensure thread safety across concurrent tasks. The table tracks:
* Writable/Readable file descriptors.
* The current offset pointer.
* Start cluster, current cluster, and file size.
* The directory slot index (used to flush size and cluster updates to disk).

---

## 7) The ATA Block I/O Layer Interface

The low-level interaction between the FAT12 file system engine and the storage medium is brokered by the kernel's advanced ATA Programmed I/O (PIO) driver.

### 7.1) Physical ATA Interface Parameters
Floppy disks are mapped as raw drive blocks. The primary ATA bus registers are accessed in Ring 0:
* **`DATA_PORT`** (`0x1F0`): Transfers sector data words (16-bit) to and from the disk controller buffer.
* **`SECTOR_COUNT_PORT`** (`0x1F2`): Configures the number of sectors to read or write in a single I/O transaction.
* **`LBA_LOW_PORT`** (`0x1F3`), **`LBA_MID_PORT`** (`0x1F4`), **`LBA_HIGH_PORT`** (`0x1F5`): Receives the target LBA bytes.
* **`DRIVE_SELECT_PORT`** (`0x1F6`): Manages drive indexing and LBA mode toggling.
* **`COMMAND_STATUS_PORT`** (`0x1F7`): Receives command codes and reports status bits.

### 7.2) Block Read/Write Control Loop
Every sector transfer cycles through a strict handshake state machine:
1. **Polling Busy**: The driver waits until the `BUSY` bit (`0x80`) is cleared.
2. **Setup**: The LBA target address and sector count are written to the port registers. The command register is then triggered.
3. **Polling Ready**: The driver loops until the `DRQ` (Data Request) status bit (`0x08`) is set.
4. **Data Transfer**: Words are read or written to the `DATA_PORT` register to transfer data.

---

## 8) Kernel-User Boundary & Syscall Dispatch (Ring 0)

User mode applications trigger filesystem operations using software interrupts via the `int 0x80` entry point.

### 8.1) System Call Register Assignment
The system call parameters are passed using standard hardware registers:
* **`rax`**: The System Call ID.
* **`rdi`**: First argument (filename string pointer or file descriptor index).
* **`rsi`**: Second argument (memory buffer pointer or FileMode enum).
* **`rdx`**: Third argument (buffer length value).

### 8.2) Security & Memory Verification
When the interrupt is triggered, the kernel switches to Ring 0 execution context. Before executing any file operation, the kernel's validation layer performs strict safety checks:
1. **String Verification (`read_user_string`)**:
   * Checks that the user-provided filename pointer (`rdi`) points to a memory range that resides completely within the user space boundaries.
   * Validates that the memory page contains a null terminator (`0`) within a size limit of 64 bytes.
2. **Buffer Verification**:
   * Verifies that the address ranges for reading (`rsi` and `rdx`) or writing (`rsi` and `rdx`) reside completely in user memory.

---

## 9) Error Handling & Recovery Strategies in Kernel and User Space

File system operations can fail at several stages, from low-level sector read errors to filesystem logic errors. A clear error mapping pipeline ensures that these failures are handled robustly.

### 9.1) The Kernel-Space `Fat12Error` Model
Within the kernel, errors are represented by the `Fat12Error` enum:
* **`NotFound`**: The requested file does not exist in the root directory.
* **`IsDirectory`**: An operation meant for a file (like write or read content) was directed at a directory.
* **`UnexpectedEof`**: The FAT chain terminated prematurely (the EOF marker was hit before the bytes specified in the directory entry were fully read).
* **`CorruptFatChain`**: An invalid cluster number was encountered (e.g. cluster ID < 2 or mapping to a bad sector marker `0xFF7`), or a circular loop was detected in the chain.
* **`NoFreeClusters`**: An append or create write operation failed because all data clusters in the FAT are in use.
* **`NoFreeDirectorySlots`**: A new file could not be created because the root directory contains no empty (`0x00`) or deleted (`0xE5`) slots.
* **`InvalidFileName`**: The file name did not conform to the 8.3 naming requirements.

### 9.2) Syscall Error Code Translation
When a filesystem syscall encounters a `Fat12Error`, the dispatcher translates the enum into a standard raw `u64` error code returned in the `rax` register:
* **`SYSCALL_ERR_UNSUPPORTED` (`u64::MAX`)**: Feature not supported.
* **`SYSCALL_ERR_INVALID_ARG` (`u64::MAX - 1`)**: Invalid arguments passed (e.g., malformed pointer or bad filename).
* **`SYSCALL_ERR_IO` (`u64::MAX - 2`)**: Input/output failure (such as disk read errors or missing files).

### 9.3) User-Space Error Mapping
In Ring 3, the safe wrappers receive these raw `u64` codes and translate them back into Rust's `Result` type. A return value greater than or equal to `u64::MAX - 2` is converted into an `Err(u64)`, allowing application code to match on errors and react safely.

---

## 10) Memory Allocation Strategy & Zero-Heap Guarantees

KAOS operates in a `#![no_std]` environment with strict limits on heap allocation. This makes the allocation strategy inside the filesystem critical for kernel stability.

### 10.1) Zero-Heap Iteration
To print a directory listing or scan for files, the kernel avoids allocating a dynamic array of directory records. Instead, the root directory parser uses a callback model:
```rust
pub fn parse_root_directory<F>(buffer: &[u8], mut on_entry: F)
where
    F: FnMut(&RootDirectoryRecord)
```
This design allows the caller to process each directory entry on-the-fly (e.g., matching a filename or displaying it to the screen) without storing them in a heap-allocated `Vec`.

### 10.2) Controlled Heap Usage
Heap allocations (via the `alloc` crate) are restricted to the global `FILE_DESCRIPTORS` list:
```rust
static FILE_DESCRIPTORS: SpinLock<Vec<FileDescriptor>> = SpinLock::new(Vec::new());
```
This vector is updated only when files are opened or closed. Because the number of concurrently open files in the kernel is small, heap fragmentation is minimized.

---

## 11) Ring 3 Safe API Architecture & Lifecycles

To provide a safe programming interface for Ring 3 applications, the raw syscall pointers are wrapped in safe abstractions in the `syscall` module.

### 11.1) The RAII `File` Handle
The file handle uses the Resource Acquisition Is Initialization (RAII) pattern:
* **The `File` Struct**: Exposes only safe, structured methods. It holds a private file descriptor integer.
* **`File::open`**: Copies the slice `&[u8]` into a stack-allocated `[u8; 64]` buffer and adds a trailing `0` to ensure it is a valid, null-terminated string before calling the kernel. This prevents buffer overflow vulnerabilities.
* **The `Drop` Trait**: The `File` struct implements `Drop`:
  ```rust
  impl Drop for File {
      fn drop(&mut self) {
          unsafe {
              let _ = raw_close_file(self.fd);
          }
      }
  }
  ```
  When the `File` variable goes out of scope (or is dropped), the destructor executes the `raw_close_file` syscall automatically.

### 11.2) Block Scoping and Automatic Drops
Because Rust destructors execute at the end of their enclosing scope, programmers can control when file handles are closed by wrapping operations in block scopes.

```text
{
    let mut file = File::open(b"test.txt", FileMode::Write)?;
    file.write(b"content")?;
    
} // <--- file goes out of scope here.
  // Destructor runs, raw_close_file is executed,
  // and the file handle is released.
```
This pattern ensures that resources are freed before subsequent operations (like deleting or reading the file) occur, preventing access and locking conflicts in the kernel.

### 11.3) The `file_exists` Helper
To determine if a file exists without locking it, the `file_exists` function opens the file in read-only mode and checks the result:
```rust
pub fn file_exists(name: &[u8]) -> bool {
    File::open(name, FileMode::Read).is_ok()
}
```
Because the return value of `File::open` is not bound to a variable, the temporary `File` object is dropped and closed immediately before `file_exists` returns. This leaves the file unlocked and ready for subsequent operations.

---

## 12) Test and Verification Architecture

The correctness of the filesystem is verified using both in-memory unit tests and QEMU-based integration tests.

### 12.1) In-Memory Unit Tests
Unit tests, located in `tests/fat12_test.rs`, bypass the physical ATA disk controller and operate on handcrafted byte arrays that mock root directory sectors and FAT tables. These tests verify:
* Filtering of deleted (`0xE5`) and LFN (`0x0F`) entries.
* Canonical normalization of 8.3 filenames (handling spaces, padding, and lowercasing).
* Traversal of short and fragmented mock FAT chains.

### 12.2) QEMU-Based Integration Tests
Integration tests run inside the QEMU emulator under a test runner script (`tests/test_runner.sh`). These tests:
1. Compile the test kernel and copy the user binaries (`hello.bin`, `readline.bin`, `filedemo.bin`) into a dynamically generated FAT12 floppy disk image.
2. Boot the image in QEMU.
3. Execute the full filesystem lifecycle: creating files, writing multi-cluster content, closing handles, reopening and reading back the content to assert equality, deleting files, and verifying that subsequent file opens fail with a NotFound error.
4. Exit QEMU via a debug port handler and report the pass/fail status to the host.

---

## 13) Comparison with C-based Legacy Implementation

The Rust-based FAT12 implementation replaces an older C-based implementation. This transition brings several structural and safety improvements:

### 13.1) Type Safety and Error Handling
* **C Implementation**: Used magic error numbers (e.g. returning `-1` or `0` for errors) and unchecked pointer arguments. This frequently led to bugs when return values were not validated.
* **Rust Implementation**: Utilizes Rust's native `Option` and `Result` types. Compile-time analysis ensures that error conditions (like `Fat12Error`) must be handled by the caller, preventing silent failures.

### 13.2) Prevention of Buffer Overflows
* **C Implementation**: Normalizing the space-padded 8.3 filenames required manual buffer iteration and inline null-termination, which was prone to off-by-one buffer overflows when dealing with malformed files.
* **Rust Implementation**: Relies on bounded slices and explicit string conversions. Array size validation is enforced by the compiler, removing the risk of out-of-bounds memory writes during filename normalization.

### 13.3) Thread Safety and Data Races
* **C Implementation**: The file descriptor table had no explicit synchronization locks, making it susceptible to race conditions when multiple tasks accessed the filesystem.
* **Rust Implementation**: The `FILE_DESCRIPTORS` list is wrapped in a thread-safe `SpinLock`. Rust's borrow checker enforces at compile time that the table cannot be accessed without acquiring the lock.

---

## 14) Physical Drive Formatting & Media Initialization Details

Floppy disk formatting configurations (like those executed during the generation of the bootable image `kaos64_rust.img` via the build tool `fat_imgen`) rely on the layout properties specified in the BIOS Parameter Block (BPB) located at physical sector 0 (LBA 0).

### 14.1) The Bios Parameter Block (BPB) Structure
```text
BPB Physical Offset Mapping:

Offset (Hex)  Field Size (Bytes)  Field Description & FAT12 Floppy Standard Values
+------------+-------------------+-------------------------------------------+
| 0x03       | 8                 | OEM Name (usually "MSDOS5.0" or similar)  |
| 0x0B       | 2                 | Bytes Per Sector (always 512)             |
| 0x0D       | 1                 | Sectors Per Cluster (always 1)            |
| 0x0E       | 2                 | Reserved Sectors count (always 1)         |
| 0x10       | 1                 | Number of FAT tables (always 2)           |
| 0x11       | 2                 | Max Root Directory Entries (always 224)   |
| 0x13       | 2                 | Total Sectors (always 2880)               |
| 0x15       | 1                 | Media Descriptor Byte (0xF0 for 1.44M)    |
| 0x16       | 2                 | Sectors Per FAT table (always 9)          |
| 0x18       | 2                 | Sectors Per Track (always 18)             |
| 0x1A       | 2                 | Number of Heads (always 2)                |
| 0x1C       | 4                 | Number of Hidden Sectors (0)              |
| 0x20       | 4                 | Large Sectors count (0 for FAT12)         |
| 0x24       | 1                 | Physical Drive Number (0x00)              |
| 0x25       | 1                 | Reserved / Flags (0x00)                   |
| 0x26       | 1                 | Extended Boot Signature (0x29)            |
| 0x27       | 4                 | Volume Serial Number                      |
| 0x2B       | 11                | Volume Label ("KAOS FLOPPY")              |
| 0x36       | 8                 | File System Type string ("FAT12   ")      |
+------------+-------------------+-------------------------------------------+
```
These parameters dictate how any standard FAT12 driver locates the filesystem headers on disk. The tool `fat_imgen` reads the boot sector `bootsector.bin`, updates these parameters, writes them to sector 0 of the output image, and then organizes sectors 1 to 32 to correspond exactly with the values defined above.

---

## 15) Detailed Control Flow Walkthrough of the Syscall Pipeline

To illustrate the integration of the Ring 3 API and the Ring 0 kernel driver, this section walks through the step-by-step execution path of a file open operation (`File::open`):

```text
 Ring 3 Application
       |
       v (Step 1)
   File::open(name: &[u8], mode: FileMode)
       |
       v (Step 2)
   Stack buffer copy & null-termination
       |
       v (Step 3)
   raw_open_file syscall wrapper
       |
       v (Step 4)
   Interrupt `int 0x80` triggered
       |
================================================= [ Privileged Ring 0 Boundary ]
       |
       v (Step 5)
   Interrupt handler & Stack swapping
       |
       v (Step 6)
   Dispatcher validation (read_user_string)
       |
       v (Step 7)
   syscall_open_file_impl
       |
       v (Step 8)
   Lock FILE_DESCRIPTORS -> Scan Root Dir -> Map LBA
       |
       v (Step 9)
   Populate FD entry & Return Index
       |
       v (Step 10)
   Restore register state & Execute `iretq`
       |
================================================= [ Unprivileged Ring 3 Boundary ]
       |
       v (Step 11)
   Result mapping -> Safe File object returned
```

### 15.1) The Step-by-Step Sequence

1. **Step 1: Ring 3 Call**: The user application executes `syscall::File::open(b"test.txt", syscall::FileMode::Write)`.
2. **Step 2: String Validation and Copy**: The safe Rust wrapper copies the slice `b"test.txt"` into a local `[u8; 64]` stack-allocated buffer and writes a null byte at index `8` (the length of the string). This guarantees that the string is null-terminated and fits within memory bounds.
3. **Step 3: Syscall Invocation**: The safe wrapper calls the unsafe function `raw_open_file(buf.as_ptr(), mode)`.
4. **Step 4: ASM Interrupt Trigger**: The assembler block loads `rax = 8` (Syscall Open ID), `rdi = pointer to stack string`, `rsi = mode (FileMode::Write as u64)`, and executes the `int 0x80` instruction.
5. **Step 5: CPU Privilege Transition**: The CPU transitions from Ring 3 to Ring 0, swaps the stack pointer (`rsp`) from the user stack to the kernel interrupt stack, saves instruction and flags register states, and jumps to the interrupt service vector registered for `0x80`.
6. **Step 6: Interrupt Handler Execution**: The kernel interrupt handler saves general-purpose registers to the stack and routes execution to the syscall dispatcher.
7. **Step 7: Pointer Sanitization**: The dispatcher receives the arguments. It executes `read_user_string` on `rdi`. The validator checks that `rdi` is below the user-mode address limit and walks the bytes until it finds a null terminator. If validation fails, it aborts the syscall and returns `SYSCALL_ERR_INVALID_ARG`.
8. **Step 8: Kernel Driver Execution**: The dispatcher calls `syscall_open_file_impl`. This function locks the `FILE_DESCRIPTORS` list via the kernel spinlock. It then calls the filesystem driver's `open_file` implementation.
9. **Step 9: Disk Read and Analysis**: The driver reads the root directory sectors via the ATA PIO interface. It scans the entries, matches `"test.txt"`, allocates a new file descriptor index, and stores file offset information in the descriptor entry.
10. **Step 10: State Return**: The dispatcher returns the file descriptor index. The interrupt handler restores register state, loads the descriptor index into `rax`, and runs the `iretq` (Interrupt Return) instruction.
11. **Step 11: Ring 3 Context Resume**: The CPU switches back to Ring 3 privilege, restores the user stack pointer, and resumes execution. The safe Rust wrapper receives the raw `rax` value, verifies it is not an error code, wraps the file descriptor inside a safe `File` object, and returns `Ok(File)`.

---

## 16) Best Practices for Ring 3 Application Developers

When building applications that target the KAOS filesystem, developers should observe the following guidelines to ensure stability, maximize I/O throughput, and avoid memory traps:

### 16.1) Path Length & Memory Restrictions
* **Path Buffer Bounds**: The stack-allocated buffer in `File::open` has a strict size limit of 64 bytes (including the trailing null terminator). Filenames that exceed 63 bytes will fail with `SYSCALL_ERR_INVALID_ARG`.
* **String Allocation**: String slices passed to filesystem APIs must be stored in reliable memory blocks (e.g. static binaries or active stack allocations). Accessing heap-allocated path buffers across async closures requires careful lifetime management to prevent dangling user pointers at the syscall boundary.

### 16.2) Explicit Block Scoping for Handles
* **Avoiding Locking Conflicts**: Because the kernel uses a cache-free model, keeping multiple file handles open for the same file in different tasks can cause state inconsistencies. Application developers should keep the lifetime of a `File` object as brief as possible.
* **Block Wrapper Pattern**: Wrap file modifications in explicit scopes `{ ... }` so the handle is dropped (and therefore closed in the kernel) before subsequent reads or deletion operations occur:
  ```rust
  // Good Practice
  {
      let mut file = File::open(b"data.bin", FileMode::Write)?;
      file.write(&buffer)?;
  } // closed here automatically
  ```

### 16.3) Error Recovery Policy
* **Checking Results**: Every filesystem call returns a `Result`. Developers should avoid using `.unwrap()` or `.expect()` in production application code, as a failed disk operation (like a missing file or full disk) will cause the entire task to panic and terminate.
* **Fallback Strategy**: Always handle the `Err` case gracefully. For example, if `File::open(..., FileMode::Read)` fails, verify whether the file was deleted or if the system encountered an LBA sector read error, and present a helpful error message to the user.
