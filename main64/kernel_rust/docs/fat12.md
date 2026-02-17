# FAT12 in This Kernel: Deep Technical Implementation Guide

This document explains the current FAT12 implementation in the Rust kernel at a level that is useful both for onboarding and for maintenance. The goal is that a reader who starts with no FAT12 background can understand the format, the design choices in this codebase, and the exact runtime behavior of both `dir` (root listing) and `read_file` (content read by 8.3 name).

The implementation described here is currently located in `src/io/fat12.rs`, is initialized from `src/main.rs`, is user-facing through the REPL command path in `src/repl.rs`, and is validated by parser-focused integration tests in `tests/fat12_test.rs`.

The current implementation now covers two read-only operations: listing root-directory entries and reading file content through FAT12 cluster-chain traversal. It still does not reconstruct long file names and does not implement write operations. This constrained scope is deliberate, because it keeps the code reviewable while already exercising the core FAT12 lookup and chain-follow logic.

---

## 1) FAT12 fundamentals and why the constants look the way they do

FAT12 organizes a volume into several consecutive regions. You can think of it as an address space with fixed sections: reserved metadata sectors at the beginning, then one or more FAT tables, then a fixed-size root-directory area, and finally the data area where file content clusters live. The key detail for this kernel is that the root directory on FAT12 is not itself a regular file chain; it lives in a dedicated fixed region.

For the 1.44 MiB floppy layout used by KAOS, the geometry is the classic one: 512 bytes per sector, one reserved sector, two FAT copies, nine sectors per FAT, and 224 root directory entries. Each root entry is 32 bytes, so the total root-directory storage is `224 * 32 = 7168` bytes. Dividing by 512 yields 14 sectors. The root directory starts right after the reserved region and FAT copies, so its first LBA is `1 + 2 * 9 = 19`. That means the root directory occupies LBAs 19 through 32 inclusive.

These values are encoded as constants in `src/io/fat12.rs` (`BYTES_PER_SECTOR`, `FAT_COUNT`, `SECTORS_PER_FAT`, `RESERVED_SECTORS`, `ROOT_DIRECTORY_ENTRIES`, `ROOT_DIRECTORY_LBA`, `ROOT_DIRECTORY_SECTORS`). The code is currently specialized to this geometry and does not parse the BPB dynamically yet.

The following ASCII diagram shows the logical on-disk layout used by this implementation:

```text
FAT12 Volume (1.44 MiB floppy profile)

LBA 0                      LBA 1                 LBA 10                LBA 19                LBA 33
|--------------------------|---------------------|---------------------|---------------------|--------->
| Reserved Sector (Boot)   | FAT #1 (9 sectors)  | FAT #2 (9 sectors)  | Root Dir (14 sec)   | Data...
| count = 1                | LBA 1..9            | LBA 10..18          | LBA 19..32          | clusters

Root Directory region size:
224 entries * 32 bytes = 7168 bytes = 14 sectors
```

You can also view this as a simple formula chain:

```text
root_dir_lba     = reserved_sectors + fat_count * sectors_per_fat
                 = 1 + 2 * 9
                 = 19

root_dir_sectors = (root_dir_entries * entry_size) / bytes_per_sector
                 = (224 * 32) / 512
                 = 14
```

---

## 2) Runtime integration in this kernel

From the kernel lifecycle point of view, FAT12 support is brought up after ATA has been initialized. In `src/main.rs`, the ATA PIO driver is initialized first, and then `io::fat12::init()` is called. At the moment, `fat12::init()` is intentionally a no-op because the implementation is cache-free and reads the root directory fresh on demand. The call is still useful as a stable lifecycle hook and makes later extension straightforward.

At the shell level, the feature is exposed by the REPL command `dir` in `src/repl.rs`. When a user enters `dir`, the REPL dispatches directly to `fat12::print_root_directory()`. That function performs the full read-parse-print sequence for the current on-disk state.

---

## 3) Module structure and separation of concerns

The FAT12 module is divided into logical stages, and this separation is one of the most important design decisions in the file.

The first stage is the disk read stage (`read_root_directory_from_disk`). It only knows how to fetch the fixed 14-sector root-directory window using ATA and return it as bytes. It does not parse entries and does not know anything about names or attributes.

The second stage is the parser (`parse_root_directory`). It consumes a byte slice, walks it in 32-byte entry units, classifies each entry according to FAT12 semantics, and emits normalized records. The parser is independent of VGA output and independent of ATA, which is why it is easy to test with synthetic buffers.

The third stage is file lookup + content read (`normalize_8_3_name`, `find_file_in_root_directory`, `read_fat_from_disk`, `fat12_next_cluster`, `read_file_from_entry`, `read_file`). This path resolves a short filename to a directory entry, validates attributes, reads FAT#1, follows the cluster chain, and returns exact file-size bytes.

The fourth stage is presentation (`print_root_directory`). It calls the reader, invokes the parser with a callback, formats each parsed record for the screen, and prints the summary line. In other words, it is strictly orchestration and display.

This split prevents the usual coupling problems (I/O mixed with parsing mixed with formatting) and gives you a clean extension path for future operations.

---

## 4) Concrete walkthrough of `src/io/fat12.rs`

This section maps the design to concrete implementation details, so you can read the source top-to-bottom and understand why each symbol exists.

At the top of `fat12.rs`, the geometry constants define the exact disk layout assumptions for the current implementation. They are intentionally hardcoded today (`512`, `2`, `9`, `1`, `224`) because the current feature is fixed to the known floppy image profile. These constants are used to derive `ROOT_DIRECTORY_LBA`, `ROOT_DIRECTORY_SECTORS`, `FAT1_LBA`, and `DATA_AREA_START_LBA`, which are the basis for both directory scanning and content reads.

The next block defines entry-local layout constants: `DIRECTORY_ENTRY_SIZE`, `ATTR_OFFSET`, `FIRST_CLUSTER_OFFSET`, and `FILE_SIZE_OFFSET`. The key design point is that the parser never depends on an implicit packed-struct memory layout. Instead, it addresses explicit offsets in a 32-byte array. This keeps the parser predictable and reviewable.

`RootDirectoryRecord` is the normalized parser output type. It is what higher layers consume, and therefore it is intentionally small: rendered short name plus length, start cluster, and size. `EntryState` is an internal parsing state machine with exactly three states (`End`, `Skip`, `Active`), which mirrors FAT12 root directory semantics.

`Fat12Error` defines the public error model for content reads. It wraps ATA driver errors and adds filesystem-level errors such as invalid short names, not found, directory-vs-file mismatches, and FAT-chain corruption cases.

`RawRootDirectoryEntry` is the raw on-disk view and acts as a tiny parsing boundary object. It has two methods. `state()` decides whether to stop parsing, skip, or decode. `parse_record()` decodes bytes into a `RootDirectoryRecord`. Keeping those two methods together makes it obvious which raw bytes participate in classification and which in field extraction.

`init()` is currently intentionally empty. That may look unusual at first, but it is a conscious lifecycle API decision: the module still exposes an initialization hook for boot sequencing while staying cache-free. So all state is read from disk at use time, not retained globally.

`read_root_directory_from_disk()` allocates one buffer sized to `ROOT_DIRECTORY_SECTORS * BYTES_PER_SECTOR` and then calls `drivers::ata::read_sectors(&mut buffer, ROOT_DIRECTORY_LBA, ROOT_DIRECTORY_SECTORS)`. `read_fat_from_disk()` performs the same pattern for FAT#1 (`SECTORS_PER_FAT` sectors at `FAT1_LBA`).

`parse_root_directory()` is the core engine. It computes `entry_count = min(ROOT_DIRECTORY_ENTRIES, buffer.len() / 32)` so malformed or short buffers cannot overrun parsing. It then iterates slot-by-slot, copies each slot into `RawRootDirectoryEntry`, classifies, decodes active entries, invokes the callback, and updates totals. The callback model is important because it decouples parsing from output policy and keeps the function easy to reuse for future APIs.

The `read_file()` path starts with `normalize_8_3_name()`, which validates and uppercases user input into the canonical FAT short-name format (`[u8; 11]`, space padded). `find_file_in_root_directory()` then searches active entries by raw short name and returns `FileEntryMeta` (attributes, first cluster, size). The function rejects directories (`ATTR_DIRECTORY`) before data reads.

`read_file_from_entry()` performs the cluster-chain traversal. It validates the initial cluster, translates cluster IDs to LBAs via `cluster_to_lba()`, reads each sector, appends only as many bytes as still needed to satisfy the exact file size, and resolves the next cluster through `fat12_next_cluster()`. Corruption defenses include invalid cluster IDs, reserved/bad cluster values, premature EOF markers, and a `visited` bitmap to detect cycles.

Finally, `print_root_directory()` orchestrates the user-visible listing. It reads fresh bytes from disk, parses them, prints each emitted record using the screen driver, and then prints the aggregate summary line. In other words, it is intentionally thin glue around `read_root_directory_from_disk()` plus `parse_root_directory()`.

---

## 5) Raw on-disk entry vs parsed record

A FAT12 root-directory slot is exactly 32 bytes on disk. In this implementation, that raw form is represented by `RawRootDirectoryEntry`, which is just a `[u8; 32]` wrapper. It exists to localize all byte-level rules in one place, including entry classification and field decoding.

The parser output is represented by `RootDirectoryRecord`. This is the semantic, code-friendly format used by the rest of the module and by tests. It contains the rendered short name buffer plus its used length, the first cluster, and the file size.

This distinction is important. If you let raw format bytes leak everywhere, higher-level code ends up repeating hard-coded offsets and format assumptions. By centralizing those details inside the raw type, the rest of the module can reason in terms of meaningful fields.

To understand the code more intuitively, it helps to see the complete 32-byte directory entry layout:

```text
FAT12/16 Short Directory Entry (32 bytes total)

Offset  Size  Field
------  ----  -----------------------------------------------
0x00    8     Name (8.3 base name, space-padded)
0x08    3     Extension (space-padded)
0x0B    1     Attributes
0x0C    1     NT reserved / case flags (implementation ignores)
0x0D    1     Creation time tenth
0x0E    2     Creation time
0x10    2     Creation date
0x12    2     Last access date
0x14    2     High word of first cluster (FAT32; 0 on FAT12)
0x16    2     Last write time
0x18    2     Last write date
0x1A    2     First cluster (low word)  <-- used by this code
0x1C    4     File size in bytes         <-- used by this code
```

In bit-level terms, the attribute byte at offset `0x0B` is:

```text
bit:   7      6       5       4       3        2       1       0
     +------+------+------+------+--------+------+------+------+
     |  R   |  R   | ARCH | DIR  | VOLUME | SYS  | HID  | RO   |
     +------+------+------+------+--------+------+------+------+

RO=ReadOnly, HID=Hidden, SYS=System, VOLUME=Volume Label,
DIR=Directory, ARCH=Archive, R=reserved.

Special case: LFN helper entries encode ATTR byte as 0x0F.
```

And this is how the 8.3 name is physically encoded:

```text
Bytes 0x00..0x07: base name, padded with spaces
Bytes 0x08..0x0A: extension, padded with spaces

Example on disk (ASCII):

"README  TXT"
 ^^^^^^--base (8 bytes incl. padding)
         ^^^--extension (3 bytes)
```

---

## 6) How entry classification works

The key classifier is `RawRootDirectoryEntry::state()`. It interprets only a few bytes, but these checks are critical for correctness.

First, if the first byte of the name field is `0x00`, FAT12 defines this as an end marker. It means there are no more valid entries in this root directory table, and parsing must stop immediately.

Second, if that first byte is `0xE5`, the entry is deleted and must be skipped.

Third, attribute filtering is applied. Attribute value `0x0F` marks an LFN helper entry, not a regular short entry. In addition, entries with the volume-ID bit set (`attr & 0x08 != 0`) represent volume labels and are not listed as normal files by this implementation.

If none of these conditions applies, the entry is treated as active and decoded.

---

## 7) Name and field decoding strategy

For active entries, `parse_record()` decodes short-name and metadata fields. The base name comes from bytes `0..8`, the extension from bytes `8..11`. Both parts are space-padded in FAT, so trailing spaces are trimmed during rendering. A dot is inserted only when an extension is present. Display output is normalized to lowercase ASCII.

For metadata, the first cluster is decoded from offsets `26..28` as little-endian `u16`, and file size from `28..32` as little-endian `u32`. The implementation uses `from_le_bytes`, which makes endianness explicit and avoids pointer-cast pitfalls.

The rendered short name is stored in a fixed `[u8; 13]` buffer together with `name_len`. The size is chosen because 8.3 format has at most 12 visible bytes (`8 + 1 + 3`), leaving one spare byte in the buffer.

The conversion pipeline can be visualized as:

```text
Raw entry bytes:
[52 45 41 44 4D 45 20 20 | 54 58 54 | ...]
  R  E  A  D  M  E ' ' ' '   T  X  T

Step 1: trim right spaces in base and extension
base = "README"
ext  = "TXT"

Step 2: concatenate with dot if ext exists
"README.TXT"

Step 3: lowercase for display policy
"readme.txt"
```

---

## 8) FAT12 12-bit FAT entry encoding (used by `read_file`)

The current code only lists root directory entries and does not yet walk cluster chains, but understanding FAT12 allocation entries is essential for the next phase. FAT12 uses 12-bit entries, which means two cluster entries are packed into three bytes.

If we call those bytes `B0`, `B1`, `B2`, then the two entries are encoded like this:

```text
Byte layout:

         B0                 B1                 B2
   +--------------+   +--------------+   +--------------+
   | 7          0 |   | 7          0 |   | 7          0 |
   +--------------+   +--------------+   +--------------+
      [ low 8 of E0 ]   [hi4 E0|lo4 E1]    [ high 8 of E1 ]

E0 (even cluster) =  B0 + ((B1 & 0x0F) << 8)
E1 (odd cluster)  = (B1 >> 4) + (B2 << 4)
```

The commonly used offset formula in FAT12 is:

```text
fat_offset = cluster + (cluster / 2)
```

From that offset, decode rule depends on cluster parity:

```text
if cluster is even:
    value =  little_endian_u16_at(fat_offset) & 0x0FFF
else:
    value = (little_endian_u16_at(fat_offset) >> 4) & 0x0FFF
```

ASCII parity view:

```text
Cluster n (even) uses:
  byte[fat_offset]      -> bits 0..7
  low nibble of next    -> bits 8..11

Cluster n (odd) uses:
  high nibble at offset -> bits 0..3
  full next byte        -> bits 4..11
```

This is exactly the logic implemented by `fat12_next_cluster()` in the current code.

---

## 9) Parser control flow in detail

`parse_root_directory(buffer, on_entry)` treats the input as a sequence of 32-byte slots. It first computes how many full entries are actually available in the provided buffer, capped by `ROOT_DIRECTORY_ENTRIES`. This protects against short input slices and also makes the parser usable in tests without requiring a full floppy image.

It then iterates entry by entry. For each index, it copies the 32-byte chunk into a local `RawRootDirectoryEntry`, asks the classifier for `End`, `Skip`, or `Active`, and branches accordingly. `End` terminates the loop, `Skip` advances to the next slot, and `Active` triggers decoding plus callback invocation.

While emitting records, the parser keeps running totals for `file_count` and `total_size`. It returns those two values at the end, so callers can print a consistent summary without re-scanning the buffer.

Because the parser uses a callback instead of building an owned collection, it stays allocation-free for the parsed path and lets the caller decide how to consume records.

---

## 10) I/O path for `dir`

When `print_root_directory()` runs, it first calls `read_root_directory_from_disk()`. That helper allocates one contiguous buffer of `14 * 512` bytes and reads sectors starting at LBA 19 through ATA (`drivers::ata::read_sectors`).

The function then runs the parser over that buffer. For each emitted `RootDirectoryRecord`, it formats one line with size, start cluster, and short name. After iteration completes, it prints a final summary line showing file count and total bytes.

The output shape mirrors the prior C implementation closely enough for operator familiarity, while the internals are now more explicit and easier to test.

---

## 11) I/O and data path for `read_file`

The content-read path starts with a user-supplied 8.3 name (for example `SFILE.TXT`). `normalize_8_3_name()` converts this into FAT short-name layout (`SFILE   TXT`) and rejects invalid forms up front. This guarantees that root-directory matching is a bytewise comparison against the on-disk short-name fields without extra heuristics.

After normalization, `read_file()` reads the root directory and resolves metadata via `find_file_in_root_directory()`. If the entry is missing, `NotFound` is returned. If the entry is a directory, `IsDirectory` is returned. Otherwise, the code reads FAT#1 and enters chain traversal.

The traversal loop in `read_file_from_entry()` reads exactly one cluster-sector at a time for this geometry and appends only the needed suffix of that sector to the output buffer, so the final vector length is exactly `file_size`. If the FAT chain ends too early (`EOF` before enough bytes) the function returns `UnexpectedEof`. If the chain is structurally invalid (bad cluster, reserved values, out-of-range offsets, loop), it returns `CorruptFatChain`.

In short, the implementation treats FAT as authoritative for continuation but treats directory `file_size` as authoritative for output length.

---

## 12) Why the implementation is intentionally cache-free

Earlier revisions experimented with storing root-directory bytes in a global cache. The current implementation deliberately removed that cache. For this stage of the project, cache-free behavior is simpler and more robust: every `dir` command reads current disk state, there is no stale metadata risk, and there is no synchronization policy to define for future write operations.

The performance trade-off is negligible in this context, because reading 14 sectors is cheap relative to the complexity and correctness burden of invalidation logic. If caching is introduced later, it should only happen together with explicit invalidation rules tied to create/delete/write paths.

---

## 13) Safety posture and `unsafe` surface

The FAT12 path here is intentionally conservative. Parsing is done from explicit byte slices and offsets rather than by reinterpreting arbitrary memory as packed structs. This avoids common alignment and aliasing hazards and keeps assumptions visible in the code.

Another consequence of the cache-free design is that the FAT12 module no longer needs custom global synchronization state for root-directory snapshots. That reduced complexity is part of the same safety strategy: fewer hidden invariants, fewer lifetime assumptions, less mutable global state.

---

## 14) Test strategy and covered contracts

`tests/fat12_test.rs` focuses on parser contracts using handcrafted in-memory root directory buffers. This gives deterministic coverage of the logic that matters most for correctness while avoiding dependency on mutable disk state.

The tests verify that deleted, LFN, and volume-label entries are excluded; that parsing stops at the first `0x00` marker; and that 8.3 names are rendered in lowercase `name.ext` form while preserving size and first-cluster values.

The new `read_file` support is covered at contract level by input-normalization tests. `test_normalize_8_3_name_returns_expected_short_name` verifies canonical FAT short-name formatting (`README  TXT`), and `test_read_file_rejects_invalid_short_name` ensures invalid multi-dot input is rejected before any disk access.

These tests match the module architecture: because parsing is isolated from I/O and printing, correctness can be validated with pure data inputs.

---

## 15) Current limitations and extension path

The implementation now supports root listing and content read for short-name files. It still does not parse BPB geometry dynamically, does not expose long filenames, does not implement recursive subdirectory traversal, and does not support write/update/delete operations.

The next technically coherent steps are to add attribute-aware presentation (for example marking directories), then dynamic geometry extraction from BPB, followed by subdirectory traversal and LFN reconstruction on top of the existing content-read path. Write support should come after read paths are stable, because write support introduces consistency rules between FAT, directory entries, and any future cache layer.

---

## 16) End-to-end trace: from REPL command to bytes on screen

When a user types `dir`, REPL command dispatch in `src/repl.rs` calls `fat12::print_root_directory()`. That function reads root-directory sectors from disk, parses 32-byte entries in order, ignores entries that are semantically not listable, decodes active 8.3 entries into `RootDirectoryRecord`, and writes formatted lines to VGA through the screen driver. Finally it prints aggregate totals.

That is the complete runtime behavior of FAT12 in the kernel today.
