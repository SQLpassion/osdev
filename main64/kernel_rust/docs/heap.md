# Heap Manager Documentation

This document describes the current heap manager implementation in
`main64/kernel_rust/src/memory/heap.rs` in detail.

## 1. Scope and Design Goals

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

## 2. High-Level Model

The heap is a contiguous virtual memory range:

- Start: `HEAP_START_OFFSET = 0xFFFF_8000_0050_0000`
- Initial size: `INITIAL_HEAP_SIZE = 0x1000` (4 KiB)
- Growth chunk: `HEAP_GROWTH = 0x1000` (4 KiB)
- Hard limit: `MAX_HEAP_SIZE = 0x0100_0000` (16 MiB)

The heap is represented as a sequence of variable-size blocks in physical
layout order.

Each block:

- Starts with a header (`HeapBlockHeader`).
- Contains payload immediately after the header.
- Stores allocation state + block size in one packed field.
- Stores `prev_size` to enable direct previous-neighbor lookup.

Free blocks additionally store intrusive free-list links inside their payload.
This enables segregated free lists without external metadata allocations.

## 3. Block Header Format

Header type:

- `HeapBlockHeader`
- `size_and_flags: usize`
- `prev_size: usize`

Bit usage in `size_and_flags`:

- Bit 0 (`IN_USE_MASK = 0x1`): allocation flag
- Bits 1..N (`SIZE_MASK = !IN_USE_MASK`): block size in bytes

The block size includes:

- Header bytes
- Payload bytes

`prev_size` stores the full size of the physically previous block.
For the first block in heap, `prev_size = 0`.

### 3.1 Header Size and Alignment

- `HEADER_SIZE = size_of::<HeapBlockHeader>()`
- With two `usize` fields this is typically 16 bytes on x86_64.
- `ALIGNMENT = align_of::<usize>()`
- On x86_64 this is typically 8 bytes.

Allocation request handling:

1. Requested payload `n`
2. Add header: `n + HEADER_SIZE`
3. Round up to `ALIGNMENT`

So allocated block size is aligned and always includes header.

### 3.2 Boundary-Tag Role of `prev_size`

`prev_size` is a lightweight boundary tag. It allows this operation in O(1):

- Given block at `addr`, previous block address is `addr - prev_size`

That avoids reverse scans during coalescing and is central to the new design.

## 4. Free-List Node Format (Intrusive Metadata)

Free blocks store `FreeListNode` at payload start:

- `prev: usize`
- `next: usize`

Important details:

- These are block-header addresses, not payload addresses.
- `0` denotes null.
- Using `usize` keeps `HeapState` trivially `Send`, which is required because
  `SpinLock<T>` only implements `Sync` for `T: Send`.

Intrusive node placement:

```text
Free block memory layout:
+------------------------+------------------------------+
| HeapBlockHeader        | FreeListNode + free payload  |
+------------------------+------------------------------+
^ block addr             ^ payload_ptr(block)
```

## 5. Segregated Free-List Topology

`HeapState` owns:

- `free_bins: [Option<usize>; FREE_BIN_COUNT]`
- `free_bin_bitmap: u64`

Where:

- `FREE_BIN_COUNT = 32`
- `free_bins[i]` points to head block of bin `i` or `None`
- `free_bin_bitmap` has bit `i` set iff bin `i` is non-empty

This gives two-level selection:

1. Compute starting size class
2. Use bitmap to find first non-empty candidate bin at/above that class

Within a chosen bin, blocks are linked as doubly linked intrusive list.

## 6. Size-Class Mapping

`size_class_index(block_size)` maps block size to a bin index via a log2-style
coarse grouping:

- Normalize with `max(block_size, MIN_FREE_BLOCK_SIZE)`
- Compute class relative to `MIN_FREE_BLOCK_SIZE`
- Clamp to `[0, FREE_BIN_COUNT - 1]`

Consequences:

- Nearby sizes are grouped.
- Small blocks remain in lower bins.
- Very large blocks collapse into highest bin.

This is not a strict buddy allocator; it is a pragmatic bucketization for fast
candidate selection.

### 6.1 Practical Bin Ranges (for typical x86_64 values)

With typical values:

- `HEADER_SIZE = 16`
- `FREE_NODE_SIZE = 16`
- `ALIGNMENT = 8`
- `MIN_FREE_BLOCK_SIZE = 32`

the class base is `log2(32) = 5`, and bin index is:

`bin = floor(log2(block_size)) - 5` (clamped to `0..31`)

So bins represent power-of-two ranges:

- Bin 0: `32..63`
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
reachable in practice (up to the class containing `16 MiB`, i.e. around bin 19
with these constants). Higher bins are harmlessly present but typically unused.

### 6.2 Why Bitmap + Bins Helps

Without bins, allocator search starts at `heap_start` and touches many unrelated
blocks. With bins:

1. Requested size computes a start bin.
2. `free_bin_bitmap` immediately skips empty bins.
3. Allocator scans only linked free blocks in relevant bins.

This removes full-heap traversal from the normal allocation path and keeps
search localized to likely-fit block groups.

## 7. Global State and Synchronization

Global container:

- `GlobalHeap`
- `inner: SpinLock<HeapState>`
- `initialized: AtomicBool`
- `serial_debug_enabled: AtomicBool`

`HeapState` fields:

- `heap_start`
- `heap_end` (exclusive)
- `free_bins`
- `free_bin_bitmap`

All heap metadata updates are serialized by `SpinLock`.
The helper `with_heap(...)` acquires the lock and gives mutable access.

## 8. Heap Memory Layout

At any time, the heap looks like this:

```text
heap_start                                                               heap_end
   |                                                                         |
   v                                                                         v
+--------+--------------------+--------+------------------+--------+----------------+
| Hdr A  | Payload A          | Hdr B  | Payload B        | Hdr C  | Payload C      |
+--------+--------------------+--------+------------------+--------+----------------+
```

Traversal by physical order:

```text
addr_0 = heap_start
addr_1 = addr_0 + size(addr_0)
addr_2 = addr_1 + size(addr_1)
...
```

Backward neighbor from boundary tag:

```text
prev_addr(current) = current - prev_size(current)
```

## 9. Initialization (`init`)

`init(debug_output)` does:

1. Compute `[heap_start, heap_end)` from constants.
2. Zero the initial heap range.
3. Reset bin heads + bitmap in `HeapState`.
4. Create one single free block covering the full initial region:
- `in_use = false`
- `size = INITIAL_HEAP_SIZE`
- `prev_size = 0`
5. Insert that block into the matching size bin.
6. Store debug flag and set initialized bit.

After `init(debug_output)`, there is exactly one free block and exactly one bin
bit set.

## 10. Allocation Path (`malloc`)

### 10.1 Steps

`malloc(size)`:

1. Save requested payload size for logging.
2. Convert to full aligned block size via
   `compute_aligned_heapblock_size(size)`.
3. Try `find_suitable_free_block(state, requested_block_size)`.
4. If found, allocate via `allocate_block(...)`.
5. If not found, compute growth amount and call `grow_heap(...)`.
6. Retry loop after successful growth.
7. Return null on overflow or bounded-growth rejection.

### 10.2 Candidate Search (`find_suitable_free_block`)

Search is bin-first, not heap-linear:

1. Compute start class index from requested size.
2. Mask bitmap with bins `>= start_idx`.
3. Iterate set bits using trailing-zero extraction.
4. Scan only blocks linked in each candidate bin.
5. Unlink and return first block with sufficient size.

Compared to prior full-heap first-fit scan, this removes the mandatory walk over
allocated blocks and unrelated free blocks in other size regions.

### 10.3 Block Split (`allocate_block`)

Given selected free block `old_size` and requested `size`:

- If `old_size >= size + MIN_SPLIT_SIZE`, split.
- Else consume full block.

Split behavior:

1. Head becomes allocated (`in_use = true`, `size = requested`).
2. Tail becomes free (`in_use = false`, `size = old_size - requested`).
3. Tail gets `prev_size = requested`.
4. Successor block (if any) gets updated `prev_size`.
5. Tail is inserted into size-appropriate free bin.

`MIN_SPLIT_SIZE = MIN_FREE_BLOCK_SIZE`, where:

- `MIN_FREE_BLOCK_SIZE = align_up(HEADER_SIZE + FREE_NODE_SIZE, ALIGNMENT)`

So every free block is guaranteed to be large enough to host its intrusive
links plus valid header/alignment constraints.

## 11. Free Path (`free`)

`free(ptr)`:

1. Return immediately for `ptr == null`.
2. Validate pointer by exact payload-match walk (`find_block_by_payload_ptr`).
3. Reject invalid pointer.
4. Reject double free (`!header.in_use()`).
5. Mark block free.
6. Coalesce with adjacent free neighbors using boundary tags.
7. Insert final coalesced block into matching bin.

This keeps free-path metadata mutations explicit and local.

## 12. Coalescing Algorithm (`coalesce_free_block`)

Coalescing is neighbor-local and O(1) in adjacency operations.

### 12.1 Previous Neighbor Merge

- Read `prev_size` from current block.
- If valid and previous block exists and is free:
- Unlink previous from its bin.
- Expand previous block size by current size.
- Use previous as new coalesced anchor.

### 12.2 Next Neighbor Merge

- Compute `next_addr = coalesced_addr + coalesced_size`.
- If next block exists and is free:
- Unlink next from bin.
- Expand coalesced block by next size.

### 12.3 Boundary-Tag Repair

After merges, update successor block's `prev_size` to new final coalesced size.

This ensures boundary tags remain consistent for future backward merges.

## 13. Heap Growth (`grow_heap`)

When no fitting free block exists:

1. Compute growth request with `compute_heap_growth_for_request(...)`
   (aligned to `HEAP_GROWTH`).
2. Reject if it would exceed `MAX_HEAP_SIZE`.
3. Determine last existing block size (needed for new block `prev_size`).
4. Append one new free block at old `heap_end`.
5. Set new block:
- `in_use = false`
- `size = amount`
- `prev_size = size_of_previous_tail_block`
6. Advance `heap_end`.
7. Coalesce appended block with free predecessor if applicable.
8. Insert final block into bins.

## 14. Pointer Conversion Helpers

Internal helpers:

- `header_at(addr)` -> `*mut HeapBlockHeader`
- `payload_ptr(block)` -> payload pointer (`block + HEADER_SIZE`)
- `free_node_ptr(block)` -> intrusive node location in payload
- `ptr_to_addr(block)` / `addr_to_ptr(addr)` -> pointer/address bridges

These functions centralize metadata addressing rules and keep call sites simple.

## 15. Logging Behavior

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

## 16. Self-Test (`run_self_test`)

The runtime self-test validates:

1. Independent allocations and payload integrity.
2. Free + reuse through larger follow-up allocation.
3. Rust allocator path via `Vec`.

Notable behavior:

- The self-test does not reinitialize a live allocator state.
- It only calls `init` when allocator is not yet initialized.

## 17. Integration Test Coverage

`main64/kernel_rust/tests/heap_test.rs` covers allocator contracts including:

- Basic alloc/free round trip.
- Reuse after free.
- Alignment for small allocations.
- Growth for large allocations and multi-growth cases.
- Overflow request rejection and post-failure usability.
- Invalid free rejection.
- Double free rejection.
- Self-test non-destructiveness.
- Global allocator and over-aligned layout behavior.
- Explicit two-sided coalescing (`prev` + `next`) behavior.

This protects the core invariants of split/coalesce/bin-link operations.

## 18. Rust Global Allocator Integration

`allocator.rs` registers:

- `#[global_allocator] GLOBAL_ALLOCATOR`
- `alloc` forwards to `heap::malloc`
- `dealloc` forwards to `heap::free`

Therefore Rust containers (`Vec`, `Box`, etc.) allocate from this heap.
Over-aligned layouts are handled by allocator-side over-allocation and a stored
back-reference to the raw heap pointer.

## 19. Safety and Invariants

Core physical layout invariants:

1. Every block header is within `[heap_start, heap_end)`.
2. Every block size is at least `HEADER_SIZE`.
3. Traversal by repeated `+ size` reaches block boundaries without overlap.
4. Traversal partitions the managed heap exactly up to `heap_end`.
5. `heap_end` always points to first byte past last block.
6. For any non-first block, `prev_size` equals size of physical predecessor.

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

Unsafe operations are localized to:

- Raw pointer arithmetic and dereference for in-place headers/nodes.
- Heap-memory initialization (`write_bytes`).

Safety depends on:

- Exclusive lock ownership during metadata updates.
- Validated block boundaries before dereference.
- Consistent boundary-tag maintenance (`prev_size`).

## 20. Performance Characteristics

Approximate characteristics:

- `malloc`: bitmap+bin search; avoids mandatory full-heap linear scan.
- `free`: pointer validation scan + O(1) neighbor coalescing + O(1) bin updates.
- Metadata overhead per block:
- Always: `HeapBlockHeader` (2 machine words)
- Free-only: intrusive `FreeListNode` in payload start

Compared to the prior first-fit full scan design:

- Better allocation scalability under mixed-size fragmentation.
- Explicitly bounded candidate-set scan by size class.

## 21. Known Limitations

1. Growth is bounded by `MAX_HEAP_SIZE`.
2. Single global lock (no per-CPU arenas).
3. No lock-free fast paths.
4. Pointer validation in `free` still uses a boundary walk.
5. No canaries/poisoning/quarantine hardening.
6. Bin strategy is coarse and may still degrade under adversarial patterns.

## 22. Worked Example

Assume x86_64-like sizes:

- `HEADER_SIZE = 16`
- `ALIGNMENT = 8`
- `FREE_NODE_SIZE = 16`
- `MIN_FREE_BLOCK_SIZE = align_up(16 + 16, 8) = 32`

Request `malloc(50)`:

1. Full size = `50 + 16 = 66`
2. Align to 8 -> `72`
3. Choose bin class for 72-byte block
4. Remove suitable free block from bin
5. Split if remainder `>= 32`
6. Return payload pointer

Block view:

```text
offset X:
+--------------------------------+
| header(size=72, in_use=1, ...) |
+--------------------------------+
| payload (50 used, padding 6)   |
+--------------------------------+
```

If split occurred, remainder at `X + 72` is a valid free block with intrusive
node at its payload start and is linked into its own size bin.

### 22.1 Detailed Bin-Aware Allocation Walkthrough

This walkthrough shows exact bin/bitmap transitions.

Initial state right after `init(false)`:

- Heap contains one free block of `4096` bytes.
- `size_class_index(4096)` -> `floor(log2(4096)) - 5 = 12 - 5 = 7`
- So only Bin 7 is populated.
- `free_bin_bitmap = 1 << 7`

State:

```text
Bin 7: [block@heap_start size=4096]
Bitmap: ...00010000000 (bit 7 set)
```

#### Step A: `malloc(50)`

1. Requested payload `50`
2. Full block size: `50 + 16 = 66`, aligned to 8 -> `72`
3. `size_class_index(72)` -> `floor(log2(72)) - 5 = 6 - 5 = 1`
4. Start bin is Bin 1; allocator checks bitmap for bins `>= 1`
5. Bin 1..6 are empty, Bin 7 is first non-empty candidate
6. Remove `4096` block from Bin 7
7. Split into:
- allocated head: `72` bytes
- free tail: `4096 - 72 = 4024` bytes
8. Insert tail into its class:
- `size_class_index(4024)` -> `floor(log2(4024)) - 5 = 11 - 5 = 6`
- tail inserted into Bin 6

State after Step A:

```text
Allocated: [72]
Bin 6: [4024]
Bitmap: ...00001000000 (bit 6 set)
```

#### Step B: `malloc(200)`

1. Requested payload `200`
2. Full block size: `200 + 16 = 216` (already 8-byte aligned)
3. `size_class_index(216)` -> `floor(log2(216)) - 5 = 7 - 5 = 2`
4. Start at Bin 2; first non-empty bin from bitmap is again Bin 6
5. Remove `4024` block from Bin 6
6. Split into:
- allocated head: `216`
- free tail: `4024 - 216 = 3808`
7. `size_class_index(3808)` -> `11 - 5 = 6`, insert tail back into Bin 6

State after Step B:

```text
Allocated: [72][216]
Bin 6: [3808]
Bitmap: ...00001000000 (bit 6 set)
```

#### Step C: `free` and Coalescing Effect on Bins

If the two allocated blocks are freed in order:

1. `free(second)` marks `216` block free, coalesces with following `3808`
   block (which is free and currently in Bin 6), removes neighbor from Bin 6,
   merges into `4024`, inserts merged block back into Bin 6.
2. `free(first)` marks `72` block free, coalesces with next `4024` block,
   removes it from Bin 6, merges to `4096`, inserts into Bin 7.

Final state returns to:

```text
Bin 7: [4096]
Bitmap: ...00010000000 (bit 7 set)
```

This sequence demonstrates the full lifecycle:

- class selection,
- bitmap skipping of empty bins,
- split + reinsert into a lower/higher class,
- coalescing with bin unlink/relink,
- restoration of large-block class after merges.

## 23. Files and Entry Points

- Implementation: `main64/kernel_rust/src/memory/heap.rs`
- Global allocator bridge: `main64/kernel_rust/src/allocator.rs`
- Integration tests: `main64/kernel_rust/tests/heap_test.rs`
- Spinlock primitive: `main64/kernel_rust/src/sync/spinlock.rs`
