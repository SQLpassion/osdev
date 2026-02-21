# Heap Manager Documentation

This document describes the current heap manager implementation in
`main64/kernel_rust/src/memory/heap.rs` in detail.

---

## 1. Conceptual Introduction

This section explains the core ideas behind the heap manager for readers who
have not worked with a segregated free-list allocator before. If you already
know the concept, skip ahead to [Section 2](#2-scope-and-design-goals).

### 1.1 The Basic Principle

The heap is a large contiguous block of memory. The allocator divides it into
variable-sized **blocks**. Each block has a small **header** (24 bytes) at its
start, followed by the usable **payload**:

```text
heap_start                                                         heap_end
  |                                                                    |
  v                                                                    v
+--------+-------------------+--------+------------------+--------+--------...
| Hdr A  | Payload A         | Hdr B  | Payload B        | Hdr C  | ...
+--------+-------------------+--------+------------------+--------+--------...
  24 B     e.g. 100 B          24 B     e.g. 200 B
```

The header stores three pieces of information:

- **Is the block in use?** (1 bit, packed into the size field)
- **How large is the block?** (block size in bytes, including the header)
- **Address-bound validation magic** (used to harden `free(ptr)` validation)

When you call `malloc(100)`, the allocator finds a free block large enough to
hold 100 bytes of payload, marks it as in-use, and returns a pointer to the
payload area. When you call `free()`, the block is marked as free again.

### 1.2 The Problem: Finding a Free Block Efficiently

The naive approach — scanning every block from `heap_start` to `heap_end` until
a large enough free block is found — is O(n). With many allocated blocks this
becomes slow.

The solution used here is a **segregated free-list**.

### 1.3 Segregated Free-List: Sorting Blocks by Size

Instead of one list of all free blocks, the allocator maintains **32 separate
lists** (called *bins*), each responsible for a different size range:

```text
Bin  0: blocks  40 –  63 bytes
Bin  1: blocks  64 – 127 bytes
Bin  2: blocks 128 – 255 bytes
Bin  3: blocks 256 – 511 bytes
...
Bin 31: very large blocks
```

The size ranges follow a logarithmic (power-of-two) scale. When `malloc(100)`
is called, the allocator goes directly to Bin 1 (64–127 bytes) and picks a
block from there — without touching any other bin.

### 1.4 How a Bin Knows Where Its Blocks Are

This is where two data structures work together.

**The bin array** stores the address of the *first* free block in each bin, or
`None` if the bin is empty:

```rust
free_bins: [Option<usize>; 32]
//  free_bins[0] = Some(0x5100) → first free block in Bin 0
//  free_bins[1] = Some(0x5200) → first free block in Bin 1
//  free_bins[3] = None         → Bin 3 is empty
```

**The free blocks themselves** know where the next block in the same bin is.
Each free block stores an intrusive `FreeListNode` directly inside its payload
area:

```rust
struct FreeListNode {
    prev: usize,   // address of previous free block in same bin
    next: usize,   // address of next free block in same bin
}
```

Because this node lives *inside the payload that is unused anyway*, no
additional memory is needed for the list structure. This is called an
**intrusive linked list**.

The full picture for Bin 1 with two free blocks:

```text
free_bins[1] = 0x5100
      │
      ▼
+─────────────────────────────+          +─────────────────────────────+
│ Header (size=80, free)      │          │ Header (size=96, free)      │
├─────────────────────────────┤          ├─────────────────────────────┤
│ FreeListNode                │          │ FreeListNode                │
│   prev = 0   (no previous)  │          │   prev = 0x5100             │
│   next = 0x5200  ───────────┼─────────►│   next = 0  (end of list)  │
├─────────────────────────────┤          ├─────────────────────────────┤
│ ... rest of payload ...     │          │ ... rest of payload ...     │
+─────────────────────────────+          +─────────────────────────────+
  Address: 0x5100                          Address: 0x5200
```

It is a doubly linked list. Following `next` pointers walks forward through the
bin; `prev` allows O(1) unlinking of any node.

### 1.4.1 How a Block Gets Into a Bin

A bin does not search for free blocks — the caller always brings the block
pointer directly to `insert_free_block`:

```rust
fn insert_free_block(state: &mut HeapState, block: *mut HeapBlockHeader)
```

The function receives the exact heap address of the block, computes its size
class, and sets `free_bins[idx]` to that address — making the block the new
list head. The bin entry is nothing more than a single stored address; there
is no search involved.

| Caller | Source of the block pointer |
|---|---|
| `init()` | `header_at(heap_start)` — the heap start address is a compile-time constant |
| `allocate_block()` | The split remainder: `block_addr + requested_size` |
| `grow_heap()` | `header_at(old_heap_end)` — the previous end of the heap |
| `free()` via `coalesce_free_block()` | The coalesced block returned directly from the coalescing step |

In every case the caller already holds the pointer. It hands it to
`insert_free_block`, which writes the block's address into `free_bins[idx]`
and links the block into the existing list:

```text
Before insert (Bin 1 has one block at 0x5200):

  free_bins[1] = Some(0x5200)

After insert_free_block(block @ 0x5100):

  free_bins[1] = Some(0x5100)   ← new head

  0x5100: FreeListNode { prev: 0,      next: 0x5200 }
  0x5200: FreeListNode { prev: 0x5100, next: 0      }
```

If the bin was empty (`None`), `next` is set to `0` (null), and the new block
becomes the only entry. The bitmap bit for that bin is set in both cases.

### 1.5 The Bitmap Optimization

Even with 32 bins, there is still the question: *which bins are non-empty?*
Iterating through all 32 bins to find the first non-empty one would still cost
up to 32 comparisons.

The solution is a single `u64` **bitmap** — one bit per bin:

```text
free_bin_bitmap: 0b...0001_1010
                           ↑↑↑
                           Bin 1 non-empty
                           Bin 3 non-empty
                           Bin 4 non-empty
```

When `malloc` needs a block of size class `k`, it masks all bits below `k` and
calls `trailing_zeros()` — a single CPU instruction (`TZCNT`) — which instantly
returns the index of the next non-empty bin at or above class `k`:

```rust
let remaining = state.free_bin_bitmap & (!0u64 << start_idx);
let idx = remaining.trailing_zeros(); // single CPU instruction
```

### 1.6 How Empty Bins Are Distinguished from Non-Empty Ones

A critical detail: the bin array uses `Option<usize>`, not a raw `usize`. An
empty bin is `None`, not zero or some sentinel address. This makes the
distinction between "empty" and "contains a block at address 0x0" explicit at
the type level — the compiler enforces it.

At initialization, **all 32 bins start as `None`**:

```rust
free_bins: [None; FREE_BIN_COUNT]
```

After `init()` runs, a single free block covering the initial heap page is
created and inserted into exactly one bin. All other 31 bins remain `None`.

When code iterates a bin:

```rust
let current = addr_to_ptr(state.free_bins[idx].unwrap_or(0));
while !current.is_null() { ... }
```

`None` becomes address `0`, which `is_null()` catches immediately, so the loop
body never executes for an empty bin.

### 1.7 Coalescing: Merging Adjacent Free Blocks

Without merging, freeing many small blocks would fragment the heap into
unusable slivers. Every `free()` call therefore **coalesces** the newly freed
block with its immediate physical neighbors if they are also free.

To merge with the *previous* block without scanning forward from `heap_start`,
each block stores the size of its physical predecessor in the `prev_size` field
of its header (**boundary tag**). This enables O(1) backward navigation:

```text
Next block:     current_addr + current_size       (forward)
Previous block: current_addr - current.prev_size  (backward, O(1))
```

After merging, the combined block is re-inserted into the bin appropriate for
its new, larger size.

---

## 2. Scope and Design Goals

The heap manager is a small kernel allocator with these goals:

- Work in `#![no_std]` context.
- Keep implementation compact and auditable.
- Keep synchronization explicit (`SpinLock`) and safe for kernel usage.
- Support Rust global allocation (`#[global_allocator]`) through `malloc/free`.
- Provide deterministic allocator behavior with explicit, bounded metadata.
- Reduce allocation-path search cost compared to linear full-heap scanning.

Non-goals in the current implementation:

- Best-in-class general-purpose allocator throughput under all workloads.
- Per-CPU heaps or lock-free behavior.
- NUMA-aware placement.
- Background compaction/defragmentation.

## 3. High-Level Model

The heap is a contiguous virtual memory range:

- Start: `HEAP_START_OFFSET = 0xFFFF_8000_0050_0000`
- Initial size: `INITIAL_HEAP_SIZE = 0x1000` (4 KiB)
- Growth chunk: `HEAP_GROWTH = 0x1000` (4 KiB)
- Hard limit: `MAX_HEAP_SIZE = 0x0100_0000` (16 MiB)

The heap is represented as a sequence of variable-size blocks in physical
layout order. Each block starts with a `HeapBlockHeader` followed by its
payload. Free blocks additionally store intrusive free-list links inside their
payload, enabling segregated free lists without external metadata allocations.

## 4. Block Header Format

Header type:

- `HeapBlockHeader`
- `size_and_flags: usize`
- `prev_size: usize`
- `magic: usize`

Bit usage in `size_and_flags`:

- Bit 0 (`IN_USE_MASK = 0x1`): allocation flag
- Bits 1..N (`SIZE_MASK = !IN_USE_MASK`): block size in bytes

The block size includes header bytes and payload bytes.

`prev_size` stores the full size of the physically previous block.
For the first block in heap, `prev_size = 0`.

`magic` stores an address-bound value derived from the header address. This is
used by `find_block_by_payload_ptr` to reject forged/corrupt headers when
validating `free(ptr)`.

### 4.1 Header Size and Alignment

- `HEADER_SIZE = size_of::<HeapBlockHeader>()`
- With three `usize` fields this is typically 24 bytes on x86_64.
- `ALIGNMENT = align_of::<usize>()`
- On x86_64 this is typically 8 bytes.

Allocation request handling:

1. Requested payload `n`
2. Add header: `n + HEADER_SIZE`
3. Round up to `ALIGNMENT`

So allocated block size is aligned and always includes the header.

### 4.2 Boundary-Tag Role of `prev_size`

`prev_size` is a lightweight boundary tag. It allows this operation in O(1):

- Given block at `addr`, previous block address is `addr - prev_size`

That avoids reverse scans during coalescing.

## 5. Free-List Node Format (Intrusive Metadata)

Free blocks store `FreeListNode` at payload start:

- `prev: usize`
- `next: usize`

Both fields are block-header addresses. `0` denotes null.

Using `usize` keeps `HeapState` trivially `Send`, which is required because
`SpinLock<T>` only implements `Sync` for `T: Send`.

Memory layout of a free block:

```text
+------------------------+------------------------------+
| HeapBlockHeader        | FreeListNode + free payload  |
+------------------------+------------------------------+
^ block addr             ^ payload_ptr(block)
```

## 6. Segregated Free-List Topology

`HeapState` owns:

- `free_bins: [Option<usize>; FREE_BIN_COUNT]`
- `free_bin_bitmap: u64`

Where:

- `FREE_BIN_COUNT = 32`
- `free_bins[i]` points to head block of bin `i` or `None`
- `free_bin_bitmap` has bit `i` set iff bin `i` is non-empty

Within a chosen bin, blocks are linked as a doubly linked intrusive list.

## 7. Size-Class Mapping

`size_class_index(block_size)` maps block size to a bin index via a log2-style
coarse grouping:

- Normalize with `max(block_size, MIN_FREE_BLOCK_SIZE)`
- Compute class relative to `MIN_FREE_BLOCK_SIZE`
- Clamp to `[0, FREE_BIN_COUNT - 1]`

This is not a strict buddy allocator; it is a pragmatic bucketization for fast
candidate selection.

### 7.1 Practical Bin Ranges (for typical x86_64 values)

With typical values:

- `HEADER_SIZE = 24`
- `FREE_NODE_SIZE = 16`
- `ALIGNMENT = 8`
- `MIN_FREE_BLOCK_SIZE = 40`

the class base is `floor(log2(40)) = 5`, and bin index is:

`bin = floor(log2(block_size)) - 5` (clamped to `0..31`)

So bins represent power-of-two ranges:

- Bin 0: `40..63`
- Bin 1: `64..127`
- Bin 2: `128..255`
- Bin 3: `256..511`
- Bin 4: `512..1023`
- Bin 5: `1024..2047`
- Bin 6: `2048..4095`
- Bin 7: `4096..8191`
- Bin 8: `8192..16383`
- ...

For the current heap limit (`MAX_HEAP_SIZE = 16 MiB`), only lower bins are
reachable in practice (up to around bin 19). Higher bins are harmlessly present
but typically unused.

## 8. Global State and Synchronization

Global container:

- `GlobalHeap`
- `inner: SpinLock<HeapState>`
- `initialized: AtomicBool`
- `serial_debug_enabled: AtomicBool`

`HeapState` fields:

- `heap_start`
- `heap_end` (exclusive)
- `tail_block_addr` — cached address of last physical block, for O(1) growth
- `free_bins`
- `free_bin_bitmap`

All heap metadata updates are serialized by `SpinLock`.
The helper `with_heap(...)` acquires the lock and gives mutable access.

## 9. Heap Memory Layout

At any time, the heap looks like this:

```text
heap_start                                                               heap_end
   |                                                                         |
   v                                                                         v
+--------+--------------------+--------+------------------+--------+----------------+
| Hdr A  | Payload A          | Hdr B  | Payload B        | Hdr C  | Payload C      |
+--------+--------------------+--------+------------------+--------+----------------+
```

Forward traversal:

```text
addr_0 = heap_start
addr_1 = addr_0 + size(addr_0)
addr_2 = addr_1 + size(addr_1)
...
```

Backward neighbor via boundary tag:

```text
prev_addr(current) = current - prev_size(current)
```

## 10. Initialization (`init`)

`init(debug_output)` does:

1. Compute `[heap_start, heap_end)` from constants.
2. Zero the initial heap range.
3. Reset bin heads + bitmap in `HeapState`.
4. Create one single free block covering the full initial region:
   - `in_use = false`
   - `size = INITIAL_HEAP_SIZE`
   - `prev_size = 0`
   - `magic = header_magic_for_addr(heap_start)`
5. Insert that block into the matching size bin.
6. Store debug flag and set initialized bit.

After `init()`, there is exactly one free block and exactly one bin bit set.
All other 31 bins remain `None`.

## 11. Allocation Path (`malloc`)

### 11.1 Steps

`malloc(size)`:

1. Save requested payload size for logging.
2. Convert to full aligned block size via
   `compute_aligned_heapblock_size(size)`.
3. Try `find_suitable_free_block(state, requested_block_size)`.
4. If found, allocate via `allocate_block(...)`.
5. If not found, compute growth amount and call `grow_heap(...)`.
6. Retry loop after successful growth.
7. Return null on overflow or bounded-growth rejection.

### 11.2 Candidate Search (`find_suitable_free_block`)

1. Compute start class index from requested size.
2. Mask bitmap with bins `>= start_idx`.
3. Iterate set bits using trailing-zero extraction (`TZCNT`).
4. Scan only blocks linked in each candidate bin.
5. Unlink and return first block with sufficient size.

### 11.3 Block Split (`allocate_block`)

Given selected free block `old_size` and requested `size`:

- If `old_size >= size + MIN_SPLIT_SIZE`, split.
- Else consume full block.

Split behavior:

1. Head becomes allocated (`in_use = true`, `size = requested`).
2. Tail becomes free (`in_use = false`, `size = old_size - requested`).
3. Tail gets `prev_size = requested`.
4. Successor block (if any) gets updated `prev_size`.
5. Tail is inserted into size-appropriate free bin.

`MIN_SPLIT_SIZE = MIN_FREE_BLOCK_SIZE = align_up(HEADER_SIZE + FREE_NODE_SIZE, ALIGNMENT)`

Every free block is therefore guaranteed to be large enough to host its
intrusive links.

## 12. Free Path (`free`)

`free(ptr)`:

1. Return immediately for `ptr == null`.
2. Validate pointer via `find_block_by_payload_ptr`.
3. Reject invalid pointer or double free (`!header.in_use()`).
4. Mark block free.
5. Coalesce with adjacent free neighbors using boundary tags.
6. Insert final coalesced block into matching bin.

`find_block_by_payload_ptr` remains O(1) and now validates:

1. `block_addr = ptr - HEADER_SIZE` stays in heap bounds.
2. Header magic matches `header_magic_for_addr(block_addr)`.
3. Header size is structurally plausible (`>= HEADER_SIZE`, aligned, in-bounds).
4. Successor boundary tag is locally consistent (`next.prev_size == block_size`), if a successor exists.

## 13. Coalescing Algorithm (`coalesce_free_block`)

Coalescing is neighbor-local and O(1) in adjacency operations.

### 13.1 Previous Neighbor Merge

- Read `prev_size` from current block.
- If valid and previous block exists and is free:
  - Unlink previous from its bin.
  - Expand previous block size by current size.
  - Use previous as new coalesced anchor.

### 13.2 Next Neighbor Merge

- Compute `next_addr = coalesced_addr + coalesced_size`.
- If next block exists and is free:
  - Unlink next from bin.
  - Expand coalesced block by next size.

### 13.3 Boundary-Tag Repair

After merges, update successor block's `prev_size` to the new final coalesced
size. This ensures boundary tags remain consistent for future backward merges.

## 14. Heap Growth (`grow_heap`)

When no fitting free block exists:

1. Compute growth request with `compute_heap_growth_for_request(...)`
   (aligned to `HEAP_GROWTH`).
2. Reject if it would exceed `MAX_HEAP_SIZE`.
3. Read tail block size in O(1) from cached `tail_block_addr`.
4. Append one new free block at old `heap_end`.
5. Set new block:
   - `in_use = false`
   - `size = amount`
   - `prev_size = size_of_previous_tail_block`
6. Advance `heap_end`.
7. Coalesce appended block with free predecessor if applicable.
8. Insert final block into bins.

## 15. Pointer Conversion Helpers

- `header_at(addr)` → `*mut HeapBlockHeader`
- `payload_ptr(block)` → payload pointer (`block + HEADER_SIZE`)
- `free_node_ptr(block)` → intrusive node location in payload
- `ptr_to_addr(block)` / `addr_to_ptr(addr)` → pointer/address bridges

These helpers centralize metadata addressing rules and keep call sites simple.

## 16. Logging Behavior

Heap logs use `logging::logln_with_options("heap", ..., serial_enabled, true)`.
`serial_enabled` is controlled by `init(debug_output)`.

Runtime toggles:

- `debug_output_enabled()` returns current heap serial debug state.
- `set_debug_output(enabled)` updates heap serial debug state and returns old value.

`malloc` log format:

```text
[HEAP] alloc ptr=0x... requested=<payload> block=<total_block_size>
```

`free` log format:

```text
[HEAP] free ptr=0x... block=<coalesced_block_size>
```

Rejected free log format:

```text
[HEAP] free rejected ptr=0x... reason=<invalid pointer|double free|corrupt block header>
```

Note: header-magic mismatches are classified as `invalid pointer`.

## 17. Self-Test (`run_self_test`)

The runtime self-test validates:

1. Independent allocations and payload integrity.
2. Free + reuse through larger follow-up allocation.
3. Rust allocator path via `Vec`.

The self-test does not reinitialize a live allocator. It only calls `init` when
the allocator is not yet initialized.

## 18. Integration Test Coverage

`main64/kernel_rust/tests/heap_test.rs` covers:

- Basic alloc/free round trip.
- Reuse after free.
- Alignment for small allocations.
- Growth for large allocations and multi-growth cases.
- Overflow request rejection and post-failure usability.
- Invalid free rejection.
- Free rejection when header magic is corrupted.
- Double free rejection.
- Self-test non-destructiveness.
- Global allocator and over-aligned layout behavior.
- Explicit two-sided coalescing (`prev` + `next`) behavior.

## 19. Rust Global Allocator Integration

`allocator.rs` registers:

- `#[global_allocator] GLOBAL_ALLOCATOR`
- `alloc` forwards to `heap::malloc`
- `dealloc` forwards to `heap::free`

Rust containers (`Vec`, `Box`, etc.) allocate from this heap.
Over-aligned layouts are handled by allocator-side over-allocation with a stored
back-reference to the raw heap pointer.

## 20. Safety and Invariants

Core physical layout invariants:

1. Every block header is within `[heap_start, heap_end)`.
2. Every block size is at least `HEADER_SIZE`.
3. Traversal by repeated `+ size` reaches block boundaries without overlap.
4. Traversal partitions the managed heap exactly up to `heap_end`.
5. `heap_end` always points to first byte past last block.
6. For any non-first block, `prev_size` equals size of physical predecessor.
7. Every block header stores `magic == header_magic_for_addr(block_addr)`.

Core free-list invariants:

1. Every free block appears in exactly one bin list.
2. No allocated block appears in any free-list bin.
3. If bin `i` is empty, bitmap bit `i` is clear.
4. If bitmap bit `i` is set, bin `i` head is non-empty.
5. Intrusive `prev/next` pointers either point to valid free blocks or null.

Core coalescing invariants:

1. `free` leaves no directly adjacent free neighbors around the freed extent.
2. Coalescing unlinks neighbor blocks before merge.
3. Successor `prev_size` is repaired after split/merge/grow transitions.

Unsafe operations are localized to raw pointer arithmetic and dereference for
in-place headers/nodes, and heap-memory initialization (`write_bytes`).

Safety depends on exclusive lock ownership during metadata updates, validated
block boundaries before dereference, and consistent boundary-tag maintenance.

## 21. Performance Characteristics

- `malloc`: bitmap+bin search via `TZCNT`; avoids full-heap linear scan.
- `free`: O(1) pointer validation + O(1) neighbor coalescing + O(1) bin updates.
- Metadata overhead per block:
  - Always: `HeapBlockHeader` (3 machine words = 24 bytes on x86_64)
  - Free-only: intrusive `FreeListNode` in payload start (2 machine words)

## 22. Known Limitations

1. Growth is bounded by `MAX_HEAP_SIZE` (16 MiB).
2. Single global lock (no per-CPU arenas).
3. No lock-free fast paths.
4. No poisoning/quarantine hardening for heap corruption detection.
5. Bin strategy is coarse and may still degrade under adversarial patterns.

## 23. Working Example

Assume x86_64 sizes: `HEADER_SIZE = 24`, `ALIGNMENT = 8`,
`FREE_NODE_SIZE = 16`, `MIN_FREE_BLOCK_SIZE = 40`.

For readability, `heap_start = 0x1000` is used instead of the real kernel
address. Each step shows the full state of heap blocks, free_bins, and bitmap.

---

### 23.1 Initial State (after `init(false)`)

`init()` zeros the first 4096-byte page, creates a single free block covering
the entire initial heap, and inserts it into Bin 7
(`size_class_index(4096) = floor(log2(4096)) - 5 = 7`).

```text
Heap layout:

  0x1000                                                           0x2000
  ┌────────────────────────────────────────────────────────────────┐
  │                      FREE  (4096 bytes)                        │
  └────────────────────────────────────────────────────────────────┘

Block @ 0x1000:
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 4096           │
  │   in_use    = false          │
  │   prev_size = 0              │
  ├──────────────────────────────┤
  │ FreeListNode                 │
  │   prev = 0  (start of list)  │
  │   next = 0  (end of list)    │
  └──────────────────────────────┘

free_bins:
  Bin 0..6:  None
  Bin 7:     Some(0x1000) ──► [0x1000] ──► (null)
  Bin 8..31: None

Bitmap:
  bit 7 set, all others 0
  [ 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 0 0 0 ]
    31                                              7             0
```

---

### 23.2 Step A: `malloc(50)`

Requested 50 bytes. Full block size: `50 + 24 = 74`, aligned to 8 → **80**.
`size_class_index(80) = 1` → allocator starts at Bin 1.
Bins 1–6 are empty (bitmap check skips them instantly).
Bin 7 is the first non-empty candidate.

The 4096-byte block is removed from Bin 7 and split:
- **Head** (80 bytes, `0x1000..0x1050`) → allocated, pointer returned to caller.
- **Tail** (4016 bytes, `0x1050..0x2000`) → `size_class_index(4016) = 6` → Bin 6.

```text
Heap layout:

  0x1000       0x1050                                             0x2000
  ┌────────────┬──────────────────────────────────────────────────┐
  │ ALLOCATED  │                 FREE  (4016 bytes)               │
  │  80 bytes  │                                                  │
  └────────────┴──────────────────────────────────────────────────┘
       │
       └── pointer returned to caller (payload at 0x1018)

Block @ 0x1000 (allocated):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 80             │
  │   in_use    = true           │
  │   prev_size = 0              │
  ├──────────────────────────────┤
  │ payload (56 bytes usable)    │ ◄── pointer returned to caller
  └──────────────────────────────┘

Block @ 0x1050 (free):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 4016           │
  │   in_use    = false          │
  │   prev_size = 80             │
  ├──────────────────────────────┤
  │ FreeListNode                 │
  │   prev = 0  (start of list)  │
  │   next = 0  (end of list)    │
  └──────────────────────────────┘

free_bins:
  Bin 0..5:  None
  Bin 6:     Some(0x1050) ──► [0x1050] ──► (null)
  Bin 7..31: None

Bitmap:
  bit 6 set, all others 0
  [ 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 0 0 ]
    31                                                6           0
```

---

### 23.3 Step B: `malloc(200)`

Requested 200 bytes. Full block size: `200 + 24 = 224`, already aligned.
`size_class_index(224) = 2` → allocator starts at Bin 2.
Bins 2–5 are empty (bitmap check). Bin 6 is the first non-empty candidate.

The 4016-byte block is removed from Bin 6 and split:
- **Head** (224 bytes, `0x1050..0x1130`) → allocated, pointer returned to caller.
- **Tail** (3792 bytes, `0x1130..0x2000`) → `size_class_index(3792) = 6` → Bin 6.

```text
Heap layout:

  0x1000       0x1050        0x1130                               0x2000
  ┌────────────┬─────────────┬────────────────────────────────────┐
  │ ALLOCATED  │  ALLOCATED  │         FREE  (3792 bytes)         │
  │  80 bytes  │  224 bytes  │                                    │
  └────────────┴─────────────┴────────────────────────────────────┘
                     │
                     └── pointer returned to caller (payload at 0x1068)

Block @ 0x1000 (allocated, unchanged):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 80             │
  │   in_use    = true           │
  │   prev_size = 0              │
  └──────────────────────────────┘

Block @ 0x1050 (allocated):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 224            │
  │   in_use    = true           │
  │   prev_size = 80             │
  ├──────────────────────────────┤
  │ payload (200 bytes usable)   │ ◄── pointer returned to caller
  └──────────────────────────────┘

Block @ 0x1130 (free):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 3792           │
  │   in_use    = false          │
  │   prev_size = 224            │
  ├──────────────────────────────┤
  │ FreeListNode                 │
  │   prev = 0  (start of list)  │
  │   next = 0  (end of list)    │
  └──────────────────────────────┘

free_bins:
  Bin 0..5:  None
  Bin 6:     Some(0x1130) ──► [0x1130] ──► (null)
  Bin 7..31: None

Bitmap:
  bit 6 set, all others 0
  [ 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 0 0 ]
    31                                                6           0
```

---

### 23.4 Step C: `free(first)` — freeing the 80-byte block at 0x1000

The block at 0x1000 (size=80) is freed first. Coalescing checks both neighbors:

- **Previous neighbor**: `prev_size = 0` → no predecessor → skip.
- **Next neighbor**: `0x1000 + 80 = 0x1050` → `in_use = true` → skip.

No merge is possible. The freed block is inserted as-is.
`size_class_index(80) = 1` → Bin 1.

**This is the first moment in the example where two bins are active at the
same time**: Bin 1 holds the small free block at 0x1000, Bin 6 still holds
the large free block at 0x1130.

```text
Heap layout:

  0x1000       0x1050        0x1130                               0x2000
  ┌────────────┬─────────────┬────────────────────────────────────┐
  │    FREE    │  ALLOCATED  │         FREE  (3792 bytes)         │
  │  80 bytes  │  224 bytes  │                                    │
  └────────────┴─────────────┴────────────────────────────────────┘

Block @ 0x1000 (free):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 80             │
  │   in_use    = false          │
  │   prev_size = 0              │
  ├──────────────────────────────┤
  │ FreeListNode                 │
  │   prev = 0  (start of list)  │
  │   next = 0  (end of list)    │
  └──────────────────────────────┘

Block @ 0x1050 (allocated, unchanged):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 224            │
  │   in_use    = true           │
  │   prev_size = 80             │
  └──────────────────────────────┘

Block @ 0x1130 (free, unchanged):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 3792           │
  │   in_use    = false          │
  │   prev_size = 224            │
  ├──────────────────────────────┤
  │ FreeListNode                 │
  │   prev = 0  (start of list)  │
  │   next = 0  (end of list)    │
  └──────────────────────────────┘

free_bins:
  Bin 0:     None
  Bin 1:     Some(0x1000) ──► [0x1000] ──► (null)    ← newly inserted
  Bin 2..5:  None
  Bin 6:     Some(0x1130) ──► [0x1130] ──► (null)
  Bin 7..31: None

Bitmap:
  bit 1 and bit 6 set
  [ 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 1 0 ]
    31                                                6         1 0
```

---

### 23.5 Step D: `free(second)` — freeing the 224-byte block at 0x1050

The block at 0x1050 (size=224) is freed. Coalescing checks both neighbors:

- **Previous neighbor**: `0x1050 - prev_size(80) = 0x1000` → `in_use = false` → **merge!**
  - Remove 0x1000 from Bin 1.
  - New size: `80 + 224 = 304`. Coalesced block at 0x1000.
- **Next neighbor**: `0x1000 + 304 = 0x1130` → `in_use = false` → **merge!**
  - Remove 0x1130 from Bin 6.
  - New size: `304 + 3792 = 4096`. Coalesced block stays at 0x1000.

Both Bin 1 and Bin 6 are now empty. Insert merged block →
`size_class_index(4096) = 7` → Bin 7.

This step demonstrates **bi-directional coalescing**: a single `free()` call
merges with both the predecessor and the successor, draining two bins at once
and restoring the heap to one contiguous free block.

```text
Heap layout:

  0x1000                                                           0x2000
  ┌────────────────────────────────────────────────────────────────┐
  │           FREE  (4096 bytes, fully restored)                   │
  └────────────────────────────────────────────────────────────────┘
       ▲              ▲                    ▲
       │              │                    │
   anchor        freed block         absorbed block
   (0x1000)       (0x1050)             (0x1130)
  merged ◄─────── triggers ──────────► merged into anchor

Block @ 0x1000 (free, after bi-directional merge):
  ┌──────────────────────────────┐
  │ Header                       │
  │   size      = 4096           │ ◄── 80 + 224 + 3792 = 4096
  │   in_use    = false          │
  │   prev_size = 0              │
  ├──────────────────────────────┤
  │ FreeListNode                 │
  │   prev = 0  (start of list)  │
  │   next = 0  (end of list)    │
  └──────────────────────────────┘

free_bins:
  Bin 0..6:  None                     ← Bin 1 and Bin 6 both emptied
  Bin 7:     Some(0x1000) ──► [0x1000] ──► (null)
  Bin 8..31: None

Bitmap:
  bit 7 set, all others 0
  [ 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 1 0 0 0 0 0 0 0 ]
    31                                              7             0
```

Heap state is identical to the initial state after `init()`. The full lifecycle
is complete: class selection, bitmap-based empty-bin skipping, splitting with
re-insertion into a lower class, two bins active simultaneously, and finally
bi-directional coalescing that drains both bins and restores the original block.

## 24. Files and Entry Points

- Implementation: `main64/kernel_rust/src/memory/heap.rs`
- Global allocator bridge: `main64/kernel_rust/src/allocator.rs`
- Integration tests: `main64/kernel_rust/tests/heap_test.rs`
- Spinlock primitive: `main64/kernel_rust/src/sync/spinlock.rs`
