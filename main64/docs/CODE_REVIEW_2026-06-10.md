# KAOS Kernel — Technical Code Review (2026-06-10)

**Scope:** Full review of `main64/kernel/src/` (memory, scheduler/sync/FPU, syscall/process,
arch/interrupts, drivers/FAT12). Focus: bugs and security vulnerabilities.
**Methodology:** Five parallel deep reviews per subsystem, each verifying the actual code paths including
assembly stubs, callers, and lock nesting. Speculative findings without a concrete failure scenario were discarded.

**Build/Test:** `cargo build` / `cargo test` from `main64/kernel/` (integration tests run as
separate kernels in QEMU, see the `[[test]]` entries in `Cargo.toml`).

---

## Instructions for autonomous processing

- Findings are sorted by priority (R-01 first). Each finding can be implemented independently.
- Per finding: set the checkbox to `[x]` once implemented and verified.
- After every fix: `cargo build` must pass cleanly; run the relevant `cargo test --test <name>`.
- Style: detailed inline doc comments following the existing kernel style, `SAFETY:` blocks on all
  unsafe operations (project convention).
- For findings that change syscall/exception behavior, check/extend the contract tests
  (`syscall_dispatch_test`, `process_contract_test`, `page_fault_death_test`).
- Findings with related context are cross-referenced (e.g. R-01 ↔ R-02). Respect ordering where stated.

---

## Priority 3 — MEDIUM

### R-04 `[ ]` Page-fault stub lacks a defensive `cli`

- **Severity:** MEDIUM · **Category:** Bug (latent)
- **File:** `src/arch/interrupts/stubs.rs:249-296` (`isr14_page_fault_stub`)

**Problem:** Every other stub begins with `cli`; the page-fault stub does not. Currently safe, because
vector 14 is an interrupt gate (`IDT_INTERRUPT_GATE`, `idt.rs:18,136`) and the CPU clears IF itself. But
correctness hinges entirely on the gate type: if vector 14 were ever switched to a trap gate (not unusual
for #PF), the handler would run with IF=1 — a timer IRQ could fire in the middle of
`populate_page_table_path` / `alloc_frame_phys` and invoke the scheduler or a nested #PF while VMM
spinlocks are held by this frame → deadlock on the non-reentrant lock. Of all handlers, the fault handler
doing the most work (allocations, page-table walks, `invlpg`) is the only one missing an explicit `cli`.

**Fix:** Add `cli` as the first instruction of `isr14_page_fault_stub` (consistency + defense in depth):
```asm
isr14_page_fault_stub:
    cli
    push rax
    ...
```

**Verification:** `cargo test --test page_fault_death_test` and `vmm_test` must remain green.

### R-05 `[ ]` FAT12 fd table is global and unowned — any task can use another task's fds

- **Severity:** MEDIUM · **Category:** Security (process isolation)
- **Files:** `src/io/fat12/fd.rs:20` (`static FILE_DESCRIPTORS`), consumers `src/syscall/dispatch/fs.rs:52/58/75/99/106`; fd allocation `fd.rs:53`

**Problem:** With `Exec` (17) + `Wait` (18), multiple user tasks coexist. The fd table is
process-agnostic: `read_file_fd`/`write_file_fd`/`close_file`/`seek_file` look up purely by the small,
predictable integer fd (`max(fd)+1`, starting at 1). Task B can pass `fd = 1` to
`WriteFile`/`SeekFile`/`CloseFile` and corrupt task A's open-file state (close A's fd out from under it,
move its offset, write through A's write handle), or read file contents through A's descriptor.

**Fix:** Store the owning task ID in `FileDescriptor` at `open_file` time (via
`scheduler::current_task_id()`) and reject `entry.owner != current_task` with `NotFound`/`InvalidArg` in
every fd operation. Additionally reap a task's fds on `Exit`/reap (leak avoidance). Note: if R-18
(task generation IDs) is implemented, include the generation in the ownership check; otherwise the slot ID
suffices for now.

**Verification:** Test: task A opens a file, task B attempts `read/write/close` on A's fd → error.

### R-06 `[ ]` `open_file` Write/Append accepts directory entries → directory gets destroyed

- **Severity:** MEDIUM · **Category:** Bug / Security
- **File:** `src/io/fat12/fd.rs:75-101` (Write), `:102-124` (Append)

**Problem:** The `Read` branch checks `attr & ATTR_DIRECTORY` (lines 57-60, → `IsDirectory`). The
`Write` branch ignores the attribute and unconditionally calls `deallocate_cluster_chain` +
`update_file_entry(.., 0, 0)` — for a subdirectory this deletes the directory and its contents.
`Append` likewise never checks. File names come from untrusted user input → filesystem corruption
triggerable from user space.

**Fix:** In both branches, reject directories after the lookup, before mutating:
```rust
if let Some((_, _, _, attr)) = entry_index {
    if attr & ATTR_DIRECTORY != 0 { return Err(Fat12Error::IsDirectory); }
}
```

**Verification:** Extend `fat12_test`: open a directory entry with `OpenMode::Write` → `IsDirectory`,
directory intact.

### R-07 `[ ]` No coherence between open fds and `delete_file`/truncate (cluster use-after-free)

- **Severity:** MEDIUM · **Category:** Bug / Security (cross-file corruption, info disclosure)
- **Files:** `src/io/fat12/fs.rs:206-247` (`delete_file`), `src/io/fat12/fd.rs:75-101` (`open_file` Write)

**Problem:** `delete_file` frees and zeroes a file's cluster chain but never consults `FILE_DESCRIPTORS`.
If the file is open, the fd afterwards points at free clusters
(`start_cluster`/`current_cluster`/`file_size` are stale). A subsequent `read_file_fd`/`write_file_fd`
reads zeroed/reallocated clusters or writes into clusters the allocator now considers free →
cross-file data corruption or disclosure of another file's freshly allocated data. Same problem with
`open(Write)` truncating an already-open file. Reachable from user space via the file syscalls.

**Fix:** Add reference awareness: before `delete_file`/`open(Write)` truncation, scan `FILE_DESCRIPTORS`
for `root_entry_index == idx` and either return a busy error or invalidate the affected fds (mark them
closed). Minimal variant: busy error.

**Verification:** Test: open file, then `delete_file` → expected busy error (or fd invalidated);
a subsequent `read` on the fd must not return foreign data.

### R-08 `[ ]` fd numbers are reused → silent wrong-file I/O after close

- **Severity:** MEDIUM · **Category:** Bug
- **File:** `src/io/fat12/fd.rs:53` (`next_fd = fds.iter().map(|fd| fd.fd).max().unwrap_or(0) + 1`)

**Problem:** After the last fd is closed the table is empty and the next `open` returns `1` again; in
general any close-then-open sequence can recycle numbers. A task that retains a numeric fd after
`close_file` and then performs I/O may operate on a different file that received the same number —
silent wrong-file read/write. No generation/validation beyond the numeric match.

**Fix:** Use a monotonically increasing global counter for fd allocation (never reuse), e.g. an
`AtomicUsize` incremented on each `open`. Alternatively a generation tag in `FileDescriptor`.

**Verification:** Test: open → close → open (different file) → I/O on the old fd number fails
and never hits the new file.

### R-09 `[ ]` No spurious-IRQ handling for IRQ7/IRQ15 — unwarranted EOIs can drop real IRQs

- **Severity:** MEDIUM · **Category:** Bug
- **Files:** `src/arch/interrupts/mod.rs:200-202` (`dispatch_irq` sends EOI for all vectors), `src/arch/interrupts/pic.rs:86-97` (`end_of_interrupt`)

**Problem:** The 8259 raises a spurious IRQ7 (master) or IRQ15 (slave) when an IRQ line deasserts before
the CPU acknowledge. Correct handling: spurious IRQ7 → no EOI at all; spurious IRQ15 → EOI to the master
only (for the cascade IRQ2), not to the slave. Detection via the In-Service Register (ISR) using OCW3.
The current code unconditionally sends EOI for every IRQ vector. An unowed EOI can prematurely acknowledge
a different, legitimately in-service interrupt → dropped IRQs (e.g. a lost ATA completion or keyboard
event), manifesting as intermittent hangs in PIO wait loops.

**Fix:** Before sending EOI for vectors 7 and 15, read the PIC ISR
(`outb(PICx_COMMAND, 0x0B); v = inb(PICx_COMMAND)`) and check the top bit. If clear → spurious:
for IRQ7 return without any EOI; for IRQ15 send EOI to the master only. Wire this into
`dispatch_irq`/`end_of_interrupt`; do not invoke the registered handler for a detected spurious IRQ.

**Verification:** Existing interrupt tests green; manual test in QEMU/Bochs (keyboard/ATA still
functional).

### R-10 `[ ]` Fatal exception path takes the serial SpinLock — deadlock instead of panic banner

- **Severity:** MEDIUM · **Category:** Bug
- **Files:** `src/arch/interrupts/handlers.rs:135-144` (`exception_handler_rust` → `serial::_debug_print`), `src/drivers/serial.rs:165-168` (`_debug_print` takes `DEBUG_SERIAL.serial.lock()`)

**Problem:** If the fault occurred while the faulting context already held the serial lock (e.g. a
#GP/#PF from inside a `debugln!`/logging call), the exception handler spins forever on a lock that will
never be released — no banner, machine hangs with no output. This is exactly the re-entrancy trap the
panic handler (`panic.rs`) already avoids with the lock-free `PanicScreenWriter`; the serial output of
the exception sink doesn't get the same treatment. (The VGA banner via `write_vga_row` is correctly
lock-free.)

**Fix:** Bypass the serial SpinLock in the fatal path — write bytes directly to the COM-port registers
with a lock-free helper (analogous to `PanicScreenWriter`), e.g. expose a `serial::force_unlocked_print`.
Never take a `SpinLock` from an unrecoverable-exception sink.

**Verification:** Visual inspection + existing tests; optionally a test provoking an exception while the
serial lock is held.

### R-11 `[ ]` User code pages are mapped writable + executable (W^X violation)

- **Severity:** MEDIUM · **Category:** Security (hardening)
- **Files:** `src/memory/vmm/mapping.rs:688-698, 736-744` (`map_user_page`); `src/process/loader.rs:146-231` (esp. `:150` `writable=true`, comment `:223-229`)

**Problem:** For `UserRegion::Code`, `map_user_page` hardcodes `no_execute = false` and takes `writable`
from the caller; the loader maps every code page with `writable = true` and deliberately never downgrades
to read-only (flat binaries mix code/.data/.bss). The net result: all loaded user code runs from pages
that are simultaneously user-accessible, writable, and executable. A memory-corruption bug in a ring-3
program can thus overwrite its own code pages and execute injected bytes — the NX protection of
stack/heap is defeated for the code region. (No kernel privilege escalation; defense in depth.)
Note: the demand-fault path (`page_fault.rs:100`) already produces code pages correctly as
read-only+executable — the loader is the divergent path.

**Fix:** Since flat binaries mix code and data, real W^X is only cleanly possible with a section-aware
loader (ELF). Until then, document/implement pragmatic options: (a) introduce an image-layout convention
(text size in a header) so that `try_map_program_image` can finalize the pure text pages after
`copy_nonoverlapping` via `set_writable(false); invlpg(va)`; or (b) document the finding as a known
limitation and enforce it via segment flags in the future ELF loader. At minimum: have the loader comment
reference this review.

**Verification:** After (a): a user program writing into its text region gets #PF (and is cleanly
terminated after R-01). `process_contract_test`/`user_mode_iretq_smoke_test` green.

### R-12 `[ ]` Heap: freed blocks < `MIN_FREE_BLOCK_SIZE` silently drop out of the free list (leak)

- **Severity:** MEDIUM · **Category:** Bug
- **File:** `src/memory/heap/types.rs:189-193` (`compute_aligned_heapblock_size`) vs. `:250-252` (`insert_free_block` guard; identical in `remove_free_block` `:283-285`)

**Problem:** `HEADER_SIZE = 24`, `FREE_NODE_SIZE = 16` → `MIN_FREE_BLOCK_SIZE = 40`. But
`compute_aligned_heapblock_size(req)` yields `align_up(req + 24, 8)`, i.e. only **32 bytes** for `req`
in `1..=8`. `allocate_block` hands out such 32-byte blocks (the split path never creates sub-40
remainders, but a full 32-byte allocation is legal). On a later `free()` without a free neighbor,
the guard `if size < MIN_FREE_BLOCK_SIZE { return; }` in `insert_free_block` triggers — the block is
silently not linked into any bin and is unfindable for `find_suitable_free_block`. Small allocations
(`Box<u8>`, `Box<u32>`, 1-8-byte layouts via `heap::malloc`) thus leak usable heap over the kernel's
lifetime; only coalescing with a coincidentally adjacent free block can recover the space.

**Fix:** Clamp the allocated block size to the minimum free-block size so every live block can
round-trip through the free list (consistent with `MIN_SPLIT_SIZE == MIN_FREE_BLOCK_SIZE`):
```rust
pub(crate) fn compute_aligned_heapblock_size(requested_size: usize) -> Option<usize> {
    requested_size
        .checked_add(HEADER_SIZE)
        .and_then(|v| align_up_checked(v, ALIGNMENT))
        .map(|v| v.max(MIN_FREE_BLOCK_SIZE))
}
```

**Verification:** `cargo test --test heap_test`; add a new test case: many 1-byte allocs +
non-adjacent frees → heap free bytes remain stable / re-allocation succeeds.

---

## Priority 4 — LOW

### R-13 `[ ]` Demand fault in the user heap window maps a supervisor+executable page

- **Severity:** LOW · **Category:** Bug / Security (latent)
- **File:** `src/memory/vmm/page_fault.rs:90-101`

**Problem:** `classify_user_region` returns `Heap` for the user heap window, but the fault handler
derives `user_access` only from `Code | Stack` (Heap is missing) and `no_execute` only from `Stack`. A
demand fault in `[USER_HEAP_BASE, USER_HEAP_END)` would map a page with `user=false` (supervisor) and
`no_execute=false` (executable) — the opposite of the heap policy in `map_user_page`. Normal operation
is unaffected because the `mmap`/`brk` syscall pre-maps heap pages (`process.rs:179-205`); but if a
ring-3 task touches an unmapped heap address, the handler installs the supervisor page, the retry faults
with P=1 → `handle_page_fault` panics the kernel (user DoS; after the R-01 fix "only" a task kill with
an inconsistent leftover mapping).

**Fix:**
```rust
let user_access = matches!(user_region, Some(UserRegion::Code | UserRegion::Stack | UserRegion::Heap));
let writable    = !matches!(user_region, Some(UserRegion::Code));
let no_execute  = matches!(user_region, Some(UserRegion::Stack | UserRegion::Heap));
```
If heap demand paging is deliberately unsupported: instead return `ProtectionFault` for
`Some(UserRegion::Heap)` so the inconsistent mapping is never created.

**Verification:** `vmm_test`/`page_fault_death_test` green; optionally a test case for a heap demand
fault.

### R-14 `[ ]` PMM metadata must reside in the identity-mapped first 4 MiB — no check enforces it

- **Severity:** LOW · **Category:** Bug (latent scaling hazard)
- **Files:** `src/memory/pmm/manager.rs:54-141` (metadata at `align_up(kernel_end_phys, PAGE_SIZE)`, accessed via raw physical addresses) + `src/memory/vmm/mod.rs:282-348` (`init` identity-maps only 4 MiB)

**Problem:** PMM header, region array, and bitmaps are placed physically after `__bss_end` and
dereferenced via their physical addresses as pointers. After the CR3 switch, low memory only exists as
the identity map of the first 4 MiB. Nothing guarantees `metadata_end <= 4 MiB`. If kernel image +
metadata ever grow past ~4 MiB, every bitmap access after `vmm::init` dereferences an unmapped address →
#PF in PMM code (under the PMM lock) → fatal. Silent today only because the kernel is small.

**Fix:** Either (a) extend the identity map in `vmm::init` up to `reserved_end` instead of the hard
4-MiB limit, or (b) add an assertion at the end of `PhysicalMemoryManager::new()`:
```rust
assert!(bitmap_base <= STACK_TOP /* == 4-MiB identity limit */,
        "PMM metadata {:#x} exceeds identity-mapped region", bitmap_base);
```

**Verification:** `pmm_test`/`vmm_test` green; the assertion variant fails loudly at boot instead of
corrupting at runtime.

### R-15 `[ ]` `RingBuffer::pop`: ABA race under multi-consumer preemption

- **Severity:** LOW · **Category:** Bug
- **File:** `src/sync/ringbuffer.rs:152-183`

**Problem:** The CAS loop guards `tail_consumer` with wrapped indices (`% N`). The CAS only checks the
index VALUE, not the slot generation: consumer A loads `tail = t` and speculatively reads `buf[t]`, gets
preempted before the CAS; while A sleeps, other consumers pop / the IRQ producer pushes exactly `k·N`
elements (N = 64 — 64 key events during a long preemption are feasible), `tail_consumer` wraps back to
`t`; A's `compare_exchange_weak(t, t+1)` SUCCEEDS → A returns the stale byte and swallows the new byte
in slot `t`. Duplicated old input + lost new input — exactly the duplicate-delivery bug the CAS was meant
to prevent. The module explicitly declares SPMC with racing consumers (lines 13-23).

**Fix:** Use free-running (non-wrapped) `usize` indices; reduce modulo `N` only when indexing the array —
a wrapped-around tail then has a different counter value and the CAS fails:
```rust
// empty: tail == head ; full: head - tail == N
let tail = self.tail_consumer.load(Acquire);
let head = self.head_producer.load(Acquire);
if tail == head { return None; }
let value = unsafe { (*self.buf.get())[tail % N] };
match self.tail_consumer.compare_exchange_weak(tail, tail.wrapping_add(1), AcqRel, Acquire) { ... }
```
Producer side analogous (`full` at `head.wrapping_sub(tail) == N`; this also recovers the previously
wasted "one empty slot"). 64-bit counter wraparound is practically unreachable.

**Verification:** `keyboard_e2e_test` green; add a unit test for wrap behavior.

### R-16 `[ ]` Debug stop path frees the currently-used kernel stack (use-after-free, test path only)

- **Severity:** LOW (debug/test builds only) · **Category:** Bug
- **File:** `src/scheduler/roundrobin/mod.rs:351-372` (`TEST_STOP_REQUESTED` branch in `on_timer_tick`)

**Problem:** The stop branch collects the stacks of ALL active slots into `stacks_to_free`
(lines 355-360) and frees them via `free_pending_stacks` (365) — without the re-queue check from
`take_pending_stacks_for_free` that protects every other path. If the stop tick interrupts a running
task (the common case in tests), `current_frame`, the GPRs saved by the IRQ stub, and the live RSP all
sit inside one of those stacks; the heap writes free-list metadata into it (and may coalesce with
neighbors) while the CPU is still executing on it — the same UAF class that c3fcca3 fixed for
`terminate_task`, re-introduced on this path.

**Fix:** Apply the same exclusion — skip the slot whose stack range contains `current_frame`:
```rust
for slot in meta.slots.iter() {
    if slot.used && !slot.stack_base.is_null()
        && !slot.is_frame_within_stack(current_frame)        // <- new
        && stacks_to_free.try_reserve(1).is_ok()
    { stacks_to_free.push((slot.stack_base, slot.stack_size)); }
}
```
(Leak the skipped stack, or park it in a debug-only deferred list.)

**Verification:** `scheduler_rr_test` (uses the stop mechanism) green and stable across multiple runs.

### R-17 `[ ]` `reset_scheduler_state` does not clear `fpu_owner` (stale owner in the next scheduler epoch)

- **Severity:** LOW (only reachable via the debug `TEST_STOP` path) · **Category:** Bug
- **File:** `src/scheduler/roundrobin/manager.rs:330-339`

**Problem:** `reset_scheduler_state` clears `slots`, `run_queue`, `running_slot`,
`pending_free_stacks` — but not `meta.fpu_owner`. After a test stop, `fpu_owner == Some(k)` from the old
epoch may remain (`initialized` is deliberately preserved, `start()` only requires
`initialized && !run_queue.is_empty()`). If slot `k` is reused by a fresh task, the owner invariant
breaks: `handle_fpu_trap` FXSAVEs the OLD epoch's live registers into the NEW task's clean FPU buffer,
or the new task at slot `k` silently inherits stale FPU/SSE registers via the
`owner == running_slot` early return.

**Fix:** Add `meta.fpu_owner = None;` to `reset_scheduler_state`; for symmetry also execute
`fpu::set_ts()` there so the next epoch starts from a defined lazy-switch state.

**Verification:** `fpu_state_test` + `scheduler_rr_test` green.

### R-18 `[ ]` `wait_for_task_exit` is keyed on reusable slot indices (latent wrong-target wait)

- **Severity:** LOW (not triggerable with the current single-spawner topology; becomes real as soon as two tasks can `Exec` concurrently) · **Category:** Bug
- **Files:** `src/scheduler/roundrobin/wait.rs:74-112` (waiter), `src/scheduler/roundrobin/spawn.rs:90-93` (first-fit slot reuse); also poll path `src/main.rs:159`

**Problem:** Task identity is the raw slot index; `spawn_internal` reuses free slots first-fit — a
just-freed slot is the most likely one for the next spawn. The liveness predicate
`task_frame_ptr(task_id).is_some()` cannot distinguish "target still alive" from "slot reused by an
unrelated task". Interleaving (needs a second spawner): waiter W blocks on slot S; S exits and is reaped
(wake issued); before W is rescheduled and re-evaluates the predicate in `sleep_if_multi`, another task
execs a new program into slot S → predicate is `true` again → W blocks until the UNRELATED task exits —
potentially forever.

**Fix:** Monotonically increasing per-spawn generation counter: `generation: u64` in `TaskEntry`
(bumped from a global `AtomicU64` in `spawn_internal`); return `(slot, generation)` (or pack them into a
single u64 PID); the wait predicate compares both via a new `task_generation(slot) -> Option<u64>`;
`remove_task` invalidates the slot's last generation. (Synergy with R-05 ownership and R-08 fd
generation.)

**Verification:** `scheduler_rr_test`/`process_contract_test` green; add a test case with slot reuse.

### R-19 `[ ]` `WriteFile` length is unbounded (disk-thrashing DoS)

- **Severity:** LOW · **Category:** Security (DoS) / Bug
- **File:** `src/syscall/dispatch/fs.rs:75-89` → `src/io/fat12/fd.rs:218-288`

**Problem:** `WriteConsole`/`WriteSerial` cap copies at `MAX_*_WRITE_LEN` (4096) — `WriteFile` does not:
`bytes_to_write = buffer.len()` is the full user-supplied length.
`WriteFile(fd, valid_ptr, 0x0000_4000_0000_0000)` (a length that still passes the range check) drives the
write loop to read user memory page by page (demand-mapped to zero) and allocate FAT clusters until the
volume is exhausted — long thrashing + guaranteed disk fill. (`ReadFile` is naturally bounded by
`file_size` (u32).)

**Fix:** Clamp `len` in `syscall_write_file_impl` to a sane `MAX_FILE_WRITE_LEN` (mirroring the console
caps) and return the clamped count — or reject oversized requests with `InvalidArg`.

**Verification:** `syscall_dispatch_test` case with a huge length → `InvalidArg`/clamp.

### R-20 `[ ]` Struct-write syscalls validate range but not alignment (UB)

- **Severity:** LOW · **Category:** Bug (UB) / Security
- **Files:** `src/syscall/dispatch/bios.rs:62, 99`, `src/syscall/dispatch/pci.rs:85` (each `out_ptr.write(value)`)

**Problem:** `is_valid_user_buffer` explicitly does not check alignment (documented at
`types.rs:241-250`). `WriteFramebuffer` correctly adds its own alignment check (`console.rs:145`) — the
three struct-returning syscalls do not. `UserDateTime` (align 4), `UserPciDevice` (align 2),
`UserBiosMemoryRegion` (align 8). `core::ptr::write` to a misaligned address is UB; the compiler may
lower large `repr(C)` stores to aligned SSE moves (`movaps`), which fault. In practice x86 usually
tolerates this — but it is UB and a latent crash.

**Fix:** Either reject misaligned pointers
(`if !(out_ptr as u64).is_multiple_of(align_of::<T>() as u64) { return Err(InvalidArg); }`, analogous to
the framebuffer path) or use `out_ptr.write_unaligned(value)`.

**Verification:** `syscall_dispatch_test` case with a misaligned `out_ptr` → `InvalidArg` (or correct
unaligned write).

### R-21 `[ ]` `cluster_to_lba` lacks an upper-bound check (corrupt FAT → phantom sectors + timeout spins)

- **Severity:** LOW · **Category:** Bug (robustness / minor DoS)
- **File:** `src/io/fat12/disk.rs:56-62`; consumers in `cluster.rs`, `fs.rs`, `fd.rs`

**Problem:** `cluster_to_lba` only rejects `cluster < 2`; the read-path validation (`fs.rs:94-103`,
`fd.rs:185,207`) accepts `2..0x0FF0` as valid. But on the 1.44 MB floppy only clusters `2..=2847` exist
(LBA < 2880). A corrupt on-disk FAT can chain to e.g. cluster 3000 → LBA 3031 beyond the disk → ATA
polls a nonexistent sector and burns the full `ATA_POLL_TIMEOUT_ITERATIONS` (10,000) per bad cluster
before returning `Timeout`. No memory unsafety (buffers are fixed 512 bytes).

**Fix:**
```rust
const MAX_DATA_CLUSTER: u16 = 2847; // (2880 - DATA_AREA_START_LBA) + 2
if cluster < FAT12_MIN_DATA_CLUSTER || cluster > MAX_DATA_CLUSTER {
    return Err(Fat12Error::CorruptFatChain);
}
```

**Verification:** `fat12_test` case with an out-of-range cluster in the FAT → immediate
`CorruptFatChain`.

### R-22 `[ ]` ATA: `sector_count == 0` unguarded (hardware interprets it as 256 sectors)

- **Severity:** LOW · **Category:** Bug (latent)
- **File:** `src/drivers/ata.rs:359-412` (`read_sectors`), `:420-473` (`write_sectors`)

**Problem:** Per the ATA spec, the value `0` in the sector-count register means 256 sectors. With
`sector_count = 0`, the buffer assert passes (`total_bytes = 0`), `setup_command` programs count 0, the
transfer loop `for sector in 0..0` does nothing, and the function returns `Ok(())` — leaving the device
armed with 256 pending transfers and pending DRQ, corrupting the next command's state. No current caller
passes 0 (FAT12 uses 1/9/14) → latent.

**Fix:** Early return before touching the controller:
```rust
if sector_count == 0 { return Ok(()); }
```

**Verification:** `ata_test` green; optionally a test case for 0.

### R-23 `[ ]` `calibrate_tsc`: polling loop without a timeout (boot hang on a broken PIT)

- **Severity:** LOW · **Category:** Bug (robustness)
- **File:** `src/drivers/time/calibration.rs:61-74`

**Problem:** The calibration loop spins until PIT channel 2 reaches 0 or exceeds 11931. If channel 2
never counts (gate misbehavior on hardware/emulator), the loop never terminates → boot hang. Also: if
the first latched read returns a tiny count (before the counter loads), the loop breaks too early and
`diff` is too small; only `cycles_per_us == 0` falls back to the default — a small-but-nonzero bad value
silently yields wrong time scaling. (Division by zero itself is correctly guarded here and in
`manager.rs:72`.)

**Fix:** Bounded iteration counter in the loop (analogous to `ATA_POLL_TIMEOUT_ITERATIONS`); on timeout
fall back to the default of 2000 cycles/µs. Additionally a plausibility window for `diff` (e.g. a minimum
value) instead of just `== 0`.

**Verification:** Boot in QEMU + Bochs still succeeds; time measurement plausible.

---

## Design observations (no immediate fixes required, document them)

| # | Topic | Detail |
|---|-------|--------|
| O-1 | No capability model | Any ring-3 program can execute `Shutdown` (`process.rs:299-302`) and `DeleteFile` on arbitrary files (`fs.rs:92-96`). Consistent with the current design; add privilege checks if multi-user ambitions arise. |
| O-2 | Kernel stacks without guard pages | `context.rs:25-52`: a stack overflow of a 64-KiB heap stack silently corrupts adjacent heap blocks instead of faulting. VMM-backed stacks with an unmapped guard page would turn this into a clean #PF. |
| O-3 | `block_task` accepts zombies | `wait.rs:18-27`: would move a Zombie to `Blocked` → slot unreapable. No current caller can do this; a one-liner guard `&& state != Zombie` closes it. |
| O-4 | Address-space teardown under the scheduler lock | `manager.rs:227-229`: `destroy_user_address_space_with_options` runs with IF=0 inside the IRQ tick — no deadlock (verified), but a latency spike; could be deferred like stack frees. |
| O-5 | Idle busy-spins | `main.rs:159` polls with `yield_now` instead of `hlt`; `idle_loop` (`main.rs:164`) is dead code. Power/CPU usage only, correctness unaffected. |
| O-6 | SMP assumptions | The lost-wakeup freedom of `sleep_if_*` and the `Sync` claims of the wait queues rest on single-core interrupt-disable atomicity (`waitqueue_adapter.rs:88-117`). Before any SMP bring-up, the queue lock must span the `block_task` call. |
| O-7 | `cursor_demo` aliases kernel text into ring 3 | `user_tasks/cursor_demo.rs:192` maps a kernel-text frame user-readable+executable. Demo-only and intentional (`release_user_code_pfns=false` prevents a kernel-frame free); keep it out of production tasks. |

---

## Explicitly checked and found correct (negative list, saves re-analysis)

- **c3fcca3 fixes complete:** sti race (IF provably stays 0 from frame selection until `iretq` on both
  the IRQ and syscall paths), lazy-FPU state loss (owner invariant holds on all production paths),
  stack free-while-in-use (re-queue protection works; the only exception is the debug path → R-16).
- **e93e5f2 fix complete:** stale `tail_block_addr` after tail merge (`types.rs:547-549`) — any coalesce
  absorbing the block at `heap_end` itself ends at `heap_end`; no analogous stale-metadata path found.
- **Heap:** double-free/UAF caught by `find_block_by_payload_ptr` (magic + bounds + boundary tag) +
  `in_use()` check; over-aligned backref arithmetic (`allocator.rs:35-65`, `generic.rs:103-166`) correct.
- **PMM:** padding bits in the last bitmap word can never be handed out.
- **VMM:** user PML4 indices (224/255) disjoint from kernel (0/256/511) — no kernel leaks into user
  intermediate tables; all freshly mapped user pages (demand fault, mmap, loader) are zeroed (no info
  leak); the scheduler switches to the kernel CR3 before PML4 teardown.
- **Syscall boundary:** `is_valid_user_buffer` correct against overflow/kernel half; register restore in
  the stub complete (only `rax` is overwritten with the result — no kernel pointer leak; RFLAGS via
  `iretq` from the task's own trap frame, no IOPL/IF escalation); `mmap` fully bounded with rollback;
  `read_user_string` with per-byte revalidation and a 128-byte bound; 8.3 normalization prevents path
  traversal; only `int 0x80` has DPL 3.
- **GDT/TSS:** 16-byte TSS descriptor correct; `TSS.RSP0` is updated on every context switch
  (`manager.rs:459` → `gdt::set_kernel_rsp0`); double fault on IST1 with a valid 16K stack;
  stack-alignment math of all stub variants correct (error-code offset 120 = 15×8).
- **PIC/port I/O:** EOI master/slave ordering correct (except spurious → R-09); asm constraints of the
  port wrappers correct.
- **Stubs/frames:** push order == `SavedRegisters` (compile-time asserts); initial task frames
  (kernel + ring 3) ABI-correct (`RSP ≡ 8 mod 16` at entry).
- **Drivers:** screen bounds checks complete, `PanicScreenWriter` lock-free; keyboard scancode lookups
  bounds-safe, no lost wakeup; ATA wait loops with timeouts, LBA-28 bound + error-bit checks present;
  PCI config math + 64-bit BAR handling correct; time monotonic, division by zero guarded.
- **FAT12:** cluster-chain cycles caught via a visited bitset; FAT accesses bounds-checked;
  directory parsing (0x00/0xE5/LFN/volume label) correct; `file_size` capped via `MAX_FILE_SIZE`.

---

## Recommended processing order

1. **R-01** (ring-3 exception → task kill) — biggest security gap, foundation for R-02/R-13 behavior.
2. **R-02** (user-pointer write check) — trivially triggerable kernel panic.
3. **R-03** (fd lock across disk I/O) — deadlock on multi-task file I/O.
4. **R-05 → R-08** (fd layer: ownership, directory guard, coherence, fd reuse) — related, best done in
   one pass.
5. **R-04, R-09, R-10, R-11, R-12** (remaining MEDIUMs, independent of each other).
6. **R-13 … R-23** (LOWs, each independent).
7. Record observations O-1…O-7 in docs/CLAUDE.md where relevant.
