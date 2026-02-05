# Heap Manager Documentation

This document describes the current heap manager implementation in
`main64/kernel_rust/src/memory/heap.rs` in detail.

## 1. Scope and Design Goals

The heap manager is a small kernel allocator with these goals:

- Work in `#![no_std]` context.
- Keep implementation compact and auditable.
- Provide deterministic behavior (first-fit, linear scan).
- Support Rust global allocation (`#[global_allocator]`) through `malloc/free`.
- Keep synchronization explicit (`SpinLock`) and safe for kernel usage.

Non-goals in the current implementation:

- Best-in-class allocation performance for large heaps.
- Advanced fragmentation control beyond split/coalesce.
- Per-CPU heaps or lock-free behavior.

## 2. High-Level Model

The heap is a contiguous virtual memory range:

- Start: `HEAP_START_OFFSET = 0xFFFF_8000_0050_0000`
- Initial size: `INITIAL_HEAP_SIZE = 0x1000` (4 KiB)
- Growth chunk: `HEAP_GROWTH = 0x1000` (4 KiB)

The heap is represented as a sequence of variable-size blocks.

Each block:

- Starts with a header (`HeapBlockHeader`).
- Contains payload immediately after the header.
- Stores allocation state and total block size in one machine word.

There are no explicit `next`/`prev` pointers. The "linking" is implicit:
the allocator computes the next block address as:

`next = current + current_block_size`

So blocks form an implicit forward chain in memory.

## 3. Block Header Format

Header type:

- `size_and_flags: usize`

Bit usage:

- Bit 0 (`IN_USE_MASK = 0x1`): allocation flag
- Bits 1..N (`SIZE_MASK = !IN_USE_MASK`): block size in bytes

The block size includes:

- Header bytes
- Payload bytes

### 3.1 Header Size and Alignment

- `HEADER_SIZE = size_of::<HeapBlockHeader>()`
- On x86_64 this is typically 8 bytes.
- `ALIGNMENT = align_of::<usize>()`
- On x86_64 this is typically 8 bytes.

Allocation request handling:

1. Requested payload `n`
2. Add header: `n + HEADER_SIZE`
3. Round up to `ALIGNMENT`

So allocated block size is always aligned and includes header.

## 4. Global State and Synchronization

Global container:

- `GlobalHeap`
- `inner: SpinLock<HeapState>`
- `initialized: AtomicBool`
- `serial_line_synced: AtomicBool`

`HeapState`:

- `heap_start`
- `heap_end` (exclusive)

All heap metadata updates are done under `SpinLock`.
The helper `with_heap(...)` acquires the lock and gives mutable access.

## 5. Heap Memory Layout

At any time, the heap looks like this:

```text
heap_start                                                      heap_end
   |                                                                |
   v                                                                v
+--------+------------------+--------+---------------+--------+-------------+
| Hdr A  | Payload A        | Hdr B  | Payload B     | Hdr C  | Payload C   |
+--------+------------------+--------+---------------+--------+-------------+
  block A size -------------------->
                                  block B size -------------->
                                                    block C size ----------->
```

Implicit forward traversal:

```text
addr_0 = heap_start
addr_1 = addr_0 + size(addr_0)
addr_2 = addr_1 + size(addr_1)
...
```

## 6. Initialization (`init`)

`init()` does:

1. Compute `[heap_start, heap_end)` from constants.
2. Zero the initial heap range.
3. Create one single free block covering the full initial region.
4. Store bounds in `HeapState`.
5. Reset `serial_line_synced` for clean logging line state.
6. Mark heap initialized.

After `init()`, layout:

```text
heap_start
   |
   v
+-------------------- one free block ---------------------------+
| Header(size = INITIAL_HEAP_SIZE, in_use = 0) | free payload  |
+---------------------------------------------------------------+
```

## 7. Allocation Path (`malloc`)

### 7.1 Steps

`malloc(size)`:

1. Save requested payload size for logging.
2. Convert to full block size: `size + HEADER_SIZE`, aligned up.
3. Search first-fit free block with `find_block`.
4. If found: `allocate_block` (split if useful), return payload pointer.
5. If not found: `grow_heap(HEAP_GROWTH)`, then retry recursively.

### 7.2 First-Fit Search (`find_block`)

Linear scan from `heap_start`:

- Read header at `current`
- Validate `block_size >= HEADER_SIZE`
- If free and `block_size >= requested_block_size`, return this block
- Else move to next block using `current += block_size`

Complexity:

- Worst-case `O(number_of_blocks)` per allocation.

### 7.3 Block Splitting (`allocate_block`)

Given free block of `old_size` and desired `size`:

- If `old_size >= size + MIN_SPLIT_SIZE`, split:
- Head becomes allocated block of `size`
- Tail becomes new free block of `old_size - size`
- Else allocate full block without split

Why `MIN_SPLIT_SIZE = HEADER_SIZE + 1`:

- Avoid creating invalid zero-size tail blocks.
- Require at least one payload byte in the remainder.

Split diagram:

```text
Before:
+------------------------------- old_size -------------------------------+
| Header(in_use=0, size=old_size) |                free                |
+-----------------------------------------------------------------------+

After (split):
+--------- size ---------+ +----------- old_size - size --------------+
| Hdr(in_use=1,size=size)| | Hdr(in_use=0,size=old_size-size) | free  |
+------------------------+ +-------------------------------------------+
```

## 8. Free Path (`free`)

`free(ptr)`:

1. If `ptr == null`, return.
2. Compute header address as `ptr - HEADER_SIZE`.
3. Mark block as free (`in_use = false`).
4. Log free operation.
5. Repeatedly call `merge_free_blocks()` until no merges remain.

Repeated merge loop ensures full coalescing in one `free` call even if
multiple adjacent free ranges become mergeable.

## 9. Coalescing (`merge_free_blocks`)

Linear pass:

1. For each block `current`, compute `next = current + size(current)`.
2. If both `current` and `next` are free, merge by:
- `current.size = current.size + next.size`
3. Continue scanning.

No backward pointers are needed because merge is done with physically adjacent
neighbors discovered by forward traversal.

Coalescing diagram:

```text
Before:
+----------- free A -----------+ +---------- free B ----------+
| Hdr(size=a, in_use=0) | ...  | | Hdr(size=b, in_use=0) | ...|
+------------------------------+ +-----------------------------+

After:
+---------------- merged free block --------------------------+
| Hdr(size=a+b, in_use=0) |                ...               |
+-------------------------------------------------------------+
```

## 10. Heap Growth (`grow_heap`)

When allocation fails due to no fitting free block:

1. Append new free block at old `heap_end` with `size = amount`.
2. Advance `heap_end` by `amount`.
3. Run merge to combine with previous trailing free block if possible.

Growth diagram:

```text
Before:
[ existing heap blocks ] [heap_end]

After append:
[ existing heap blocks ][ New free block(size=HEAP_GROWTH) ][new heap_end]

After merge:
If last old block was free, it coalesces with new block.
```

## 11. Pointer Conversion Helpers

- `header_at(addr)` -> `*mut HeapBlockHeader`
- `payload_ptr(block)` -> `block + HEADER_SIZE`
- `block_from_payload(ptr)` -> `ptr - HEADER_SIZE`

These are central to map between allocator-internal metadata and public
payload pointers.

## 12. Logging Behavior

`heap_logln(...)` writes through central logging subsystem.

`malloc` log format:

```text
[heap] alloc ptr=0x... requested=<payload> block=<total_block_size>
```

`free` log format:

```text
[heap] free ptr=0x... block=<total_block_size>
```

`serial_line_synced` ensures first heap log after `init()` starts on a fresh
line to avoid formatting collisions with test harness output.

## 13. Self-Test (`run_self_test`)

The self-test validates:

1. Initial allocation layout
2. Free behavior of first block
3. Split behavior for small allocation
4. Allocation in split remainder
5. Merge behavior after full free
6. Rust allocator path via `Vec` allocation

`read_heapblock_metadata(base, offset)` inspects `(size, in_use)` for a block
at `base + offset` and is only used for layout assertions in the self-test.

## 14. Rust Global Allocator Integration

`allocator.rs` registers:

- `#[global_allocator] GLOBAL_ALLOCATOR`
- `alloc` forwards to `heap::malloc`
- `dealloc` forwards to `heap::free`

Therefore Rust containers (`Vec`, `Box`, etc.) allocate from this heap
when alignment constraints are supported.

## 15. Safety and Invariants

Core invariants:

1. Every block header is within `[heap_start, heap_end)`.
2. Every block size is at least `HEADER_SIZE`.
3. Block traversal by repeated `+ size` never overlaps headers.
4. Sum of traversed block sizes covers managed heap range.
5. No two adjacent free blocks remain after `free` completes.
6. `heap_end` always points to first byte past the last block.

Unsafe operations are localized to:

- Raw pointer arithmetic and dereference for in-place headers.
- Memory initialization (`write_bytes`).

Safety relies on:

- Correct heap bounds.
- Valid block structure.
- Exclusive state access via spinlock.

## 16. Performance Characteristics

- `malloc`: `O(n)` scan in number of blocks.
- `free`: `O(n)` for repeated merge passes in worst case.
- Memory overhead: one header per block.

This is acceptable for early-stage kernel usage, but not optimized for
large, highly fragmented heaps.

## 17. Known Limitations

1. Recursive retry in `malloc` after growth.
2. No dedicated OOM policy (null return/panic strategy is minimal).
3. No per-CPU arenas.
4. No segregated free lists.
5. Debug hardening (canaries, double-free detection) is limited.

## 18. Worked Example

Assume 64-bit target (`HEADER_SIZE = 8`, `ALIGNMENT = 8`).

Request `malloc(50)`:

1. Full size = `50 + 8 = 58`
2. Align up to 8 -> `64`
3. First suitable free block found and split if large enough

Resulting allocated block:

```text
offset X:
+-------------------------------+
| header(size=64, in_use=1)     |
+-------------------------------+
| payload (50 used, padding 6)  |
+-------------------------------+
```

Remaining free tail starts at `X + 64`.

## 19. Files and Entry Points

- Implementation: `main64/kernel_rust/src/memory/heap.rs`
- Global allocator bridge: `main64/kernel_rust/src/allocator.rs`
- Integration tests: `main64/kernel_rust/tests/heap_test.rs`
- Spinlock primitive: `main64/kernel_rust/src/sync/spinlock.rs`

