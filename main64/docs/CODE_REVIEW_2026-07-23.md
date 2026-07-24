# KAOS Kernel — Code Review & Implementation Backlog (2026-07-23)

> **Purpose of this document.** This is a full-source code review of the KAOS kernel
> (`main64/kernel/src/`) written to be **actionable by another engineer or AI agent**.
> Every finding lists the affected files with line anchors, the mechanism/root cause, a
> reproduction idea, a concrete step-by-step fix plan, and acceptance criteria. Line
> numbers reflect the tree as of commit `5cf290a` (2026-07-23) and may drift; treat them
> as starting points, confirm against the current source before editing.
>
> **How to use it.** Work top-down through §4 (Critical) → §5 (High) → §6 (Medium). Each
> item is self-contained. Before fixing #C1 and #C2, write the red test described in the
> item to confirm the defect, then make it green. §8 is the forward-looking feature
> roadmap (do this *after* the critical bugs are closed).

---

## 1. Overall assessment

KAOS is a mature, well-engineered hobby/learning kernel: ~21.5K lines of kernel + ~10.7K
lines of integration tests (34 QEMU-booted binaries), a unified BIOS+UEFI boot path, a
higher-half design with a recursive-mapping VMM, ring-3 user space with a syscall layer,
lazy FPU switching, AHCI + ATA + FAT32 + GPT, and PCI enumeration.

**Standout strengths**

- `SAFETY:` rationale on every `unsafe`; per-subsystem `docs/*.md`; boot-ordering comments
  that explain *why*.
- Strong test culture: custom `no_std` framework, panic-contract tests, concurrency tests,
  layout tests, death tests.
- Clean, testable layering; arch dependencies hidden behind trait callbacks.

**Grade**

- As a learning/hobby OS and Rust `no_std` artifact: **A− (very strong).**
- Judged as a production, multi-core, security-hardened OS: **C+ (early)** — expected; the
  gaps are architectural (single-core assumptions, no copy-in/out fault fixups, read-only
  FS), not sloppiness.

The review found **two serious latent defects** (one security, one deadlock), a cluster of
**AHCI** correctness bugs, and a set of medium robustness issues. These are subtle,
adversarial-input / real-multicore-hardware issues — not "the code is bad."

**The load-bearing invariant of the whole kernel:** *single core + interrupts disabled in
critical sections* (the `#PF`, IRQ, and scheduler paths are interrupt gates / run under
`cli`). A large fraction of the "correct today" properties below depend on it. It is mostly
**undocumented at the point of use**, and it is what every SMP effort must dismantle
carefully.

---

## 1a. Recorded design decisions

> Decisions taken by the maintainer that shape the backlog below. Kept here so a future
> implementer understands *why* certain "expected" features are intentionally absent.

- **DD-1 (2026-07-24) — Process creation uses `spawn`, not `fork`.** KAOS deliberately does
  **not** adopt the Unix `fork()`+`exec()` model. New processes are created by a
  `spawn(image, args)`-style syscall that builds a **fresh** address space and loads a new
  program image (the model Windows `CreateProcess` / POSIX `posix_spawn` use), extending the
  existing `process::exec_from_image` machinery. **Consequences for this document:**
  - **Copy-on-write is out of scope.** CoW's only real driver in KAOS would have been
    `fork()`; with `spawn` there is no parent address space to share, so no CoW is needed.
    Roadmap #6 is rewritten accordingly (see §8).
  - **Frame refcounting (finding M3) is downgraded, not deleted.** Its CoW/fork rationale is
    gone, so it is no longer a prerequisite for anything on the roadmap. The residual value
    (retire the fragile kernel-text/user-code alias workaround; reclaim non-Code/Stack/Heap
    regions on teardown) is real but *optional cleanup* now, and independent of the spawn
    decision. It would only return as a **requirement** if the kernel later chooses to share
    read-only code pages between multiple instances of the *same* binary (a memory
    optimisation, cf. how Windows shares DLLs) — which `spawn` does not need for correctness.

---

## 2. Severity legend

| Level | Meaning |
|-------|---------|
| **CRITICAL** | User-triggerable kernel compromise/crash, or a latent hard hang reachable on the supported target. Fix before "trusted ring-3." |
| **HIGH** | Correctness/data-integrity bug reachable on real hardware or hostile input; or a design flaw that will deadlock/corrupt under a planned near-term feature (SMP, writes). |
| **MEDIUM** | Robustness/perf/clarity defect; wrong or misleading invariant; fragile-but-currently-safe. |
| **LOW** | Cosmetic, cleanup, or nice-to-have. |

---

## 3. Findings index

| ID | Sev | Area | One-liner |
|----|-----|------|-----------|
| H2 | HIGH | drivers/serial | Unbounded TX busy-wait, also on the panic path |
| H3 | HIGH | scheduler | Address-space teardown runs under `SCHED` lock with IF=0 (`SCHED→PMM` lock order) |
| H4 | HIGH | io/vfs | `vfs::with()` derefs a pointer after dropping the lock; `mount`/`reset_mounted_fs` un-gated (UAF by convention) |
| H5 | HIGH | io/fat32 | No cluster-chain cycle detection → up to 1e6 real sector reads (DoS on hostile media) |
| H6 | HIGH | drivers/ata | Missing 400 ns post-command settle (stale-status race) + no post-write completion/flush |
| M1 | MEDIUM | memory/vmm | VMM lock docs claim multi-core synchronization it does not provide |
| M2 | MEDIUM | memory/vmm | Kernel-space non-present faults silently demand-map fresh RW pages (masks wild pointers) |
| M3 | LOW *(was MEDIUM; deprioritized by DD-1)* | memory | No physical-frame refcounting; shared frames handled by a fragile boolean workaround — optional cleanup only now that `fork`/CoW is out of scope |
| M4 | MEDIUM | console | Interrupt-disabling lock held across full-screen VRAM flush (latency spike on scroll) |
| M5 | MEDIUM | io/gpt | `fallback_esp()` returns `Some(2048)` on any failure → misleading downstream error |
| M6 | MEDIUM | syscall | No per-syscall authorization (`Shutdown`/`Exec`/cross-task `Wait` open to all ring-3) |
| M7 | MEDIUM | scheduler | `yield_now` routes through `int 0x20` → PIC EOI with no IRQ in service |
| M8 | MEDIUM | syscall | `user.rs` decode boilerplate duplicated ~40×; `sys_yield` threshold misclassifies Io/OOM as success |
| M9 | MEDIUM | memory/heap | Freed kernel-heap blocks not scrubbed; no reuse-zeroing (info-leak surface) |
| L1..L8 | LOW | various | See §7 |

---

## 5. HIGH findings

### H2 — Unbounded serial TX busy-wait (also on panic path)

**Severity:** HIGH. **Area:** `drivers/serial.rs`.

**Affected code.** `kernel/src/drivers/serial.rs:98` `while !self.is_transmit_empty() {
spin_loop() }` — no timeout. Also reached from the panic printer
`force_unlocked_print` (`serial.rs:178`).

**Impact.** A stuck/absent UART hangs the writer forever; on the panic path a dead UART
hangs the panic printer, so you get *no* diagnostic on real hardware.

**Fix plan.** Bound the wait with an iteration cap (mirror `ATA_POLL_TIMEOUT_ITERATIONS`,
`ata.rs:67`). On timeout, drop the byte and return (serial is best-effort debug output).
Keep the cap generous enough not to truncate normal output under QEMU.

**Acceptance criteria.** `tests/serial_deadlock_test.rs` extended (or a new case) proving the
writer returns within a bounded loop when the LSR never reports transmit-empty.

---

### H3 — Address-space teardown under the `SCHED` lock with interrupts disabled

**Severity:** HIGH (latency + future deadlock). **Area:** `scheduler/roundrobin/manager.rs`.

**Affected code.** `reap_zombies` (`manager.rs:276`) → `remove_task` →
`destroy_user_address_space` (`manager.rs:228`) walks page tables and returns frames to the
PMM (**taking the PMM lock**) and the heap — all while holding `SCHED` with `IF=0`.

**Impact.** Unbounded interrupt-latency spike on task exit; and it establishes a
`SCHED → PMM` / `SCHED → HEAP` lock order that any future SMP or reverse-order path will
deadlock on.

**Fix plan.** Split reaping into two phases (the codebase already uses this pattern for stack
frees, `manager.rs:491`): under `SCHED`, unlink the zombie and move its address-space handle
into a pending-destroy list; **after** releasing `SCHED` (and with interrupts restored to the
caller's prior state), perform the page-table walk + PMM/heap frees. Keep the `try_reserve`/
bounded-leak-on-OOM discipline.

**Acceptance criteria.** No PMM/heap lock is acquired while `SCHED` is held; measured (or
reasoned) interrupt-off window during exit is bounded; `scheduler_rr_test` still green.

---

### H4 — `vfs::with()` use-after-free by convention; `mount`/`reset_mounted_fs` un-gated

**Severity:** HIGH (soundness). **Area:** `io/vfs.rs`.

**Affected code.** `kernel/src/io/vfs.rs:82` `with()` copies a `*const dyn FileSystem` out
from under the `MOUNTED_FS` lock, drops the guard, then `unsafe { &*ptr }`. Both `mount()`
(`vfs.rs:69`) and `reset_mounted_fs()` (`vfs.rs:162`) are `pub`/un-gated and drop the `Box`.

**Impact.** If `mount`/`reset` ran concurrently with an in-flight `with()` closure, the
pointer dangles (UAF). Safe today only because both are called at boot / in tests.

**Fix plan.**
1. Gate `reset_mounted_fs` behind `#[cfg(test)]`.
2. Make the `with()` invariant explicit and enforced: either (a) hold the lock across the
   closure (acceptable only if no closure does blocking I/O — audit callers; FAT32 reads the
   whole file before taking its own fd lock, so re-check), or (b) keep the current
   drop-then-deref but document and enforce "mount is write-once at boot; never replaced
   while the system runs," e.g. by making the slot a `OnceCell`-style write-once.
3. Longer term this is subsumed by the real VFS/mount-table work (roadmap #3).

**Acceptance criteria.** No public API can free the mounted FS while a `with()` borrow is
outstanding under the documented usage; `reset_mounted_fs` not reachable from non-test code.

---

### H5 — FAT32 cluster chains have no cycle detection (1e6-read DoS)

**Severity:** HIGH (DoS on hostile/corrupt media). **Area:** `io/fat32.rs`.

**Affected code.** Chain-follow loops guarded only by a 1,000,000 iteration counter:
`fat32.rs:167` (read_file), `:248`, `:295` (open). Per-entry checks (`next_cluster`,
`fat32.rs:433`) validate range/EOC/bad-cluster but not revisits.

**Impact.** A crafted 2-cluster cycle (A→B→A) passes every per-entry check and spins doing
up to ~1e6 real sector reads before `BadChain`.

**Fix plan.** Bound the chain length by the volume's actual cluster count (already derived as
`max_data_cluster`, `fat32.rs:117`): a chain longer than `cluster_count` is necessarily
cyclic → return `BadChain` immediately. (A visited-set is unnecessary and needs allocation;
the length bound is O(1) space and strictly correct.)

**Acceptance criteria.** New `tests/fat32_test.rs` case with a synthetic cyclic FAT returns
`BadChain` in ≤ `cluster_count` iterations (no giant read storm).

---

### H6 — ATA missing 400 ns settle and post-write completion/flush

**Severity:** HIGH (real-HW correctness / write integrity). **Area:** `drivers/ata.rs`.

**Affected code.** `setup_command` writes the command byte (`ata.rs:408`) then immediately
enters `wait_ready_or_error` (`ata.rs:413`); the first status sample can observe stale
`!BSY && DRQ` from the prior state (`ata.rs:247`, `:282`). `write_sectors` (`ata.rs:477`) has
no final BSY-clear/error check after the last sector and issues no `CACHE FLUSH` (0xE7).

**Impact.** On fast real CPUs the driver can start reading before data is ready; write errors
on the final sector and drive-write-cache contents go undetected/unflushed — a data-integrity
gap once writes matter.

**Fix plan.**
1. After writing the command byte, insert the spec-mandated settle: read the **alternate
   status** register 4× (≈400 ns) before sampling real status.
2. In `write_sectors`, after the last sector: wait for BSY clear, check ERR, then issue
   `CACHE FLUSH` (0xE7) and wait for its completion. Surface errors as `BlockError`.

**Acceptance criteria.** `tests/ata_test.rs` write path checks final status + issues flush;
read path inserts the alternate-status settle (verify via the test's mock/QEMU behavior).

---

## 6. MEDIUM findings

### M1 — VMM lock advertises multi-core synchronization it does not provide
`kernel/src/memory/vmm/mod.rs:16`, `:99` claim "synchronized multi-core access / prevent
race conditions when multiple tasks map/unmap concurrently," but `with_vmm` guards only the
`VmmState` scalar fields; every real page-table edit in `mapping.rs`/`page_fault.rs` is
lock-free. Actual model: single core + IF-disabled (`#PF` is an interrupt gate). **Fix:**
correct the doc comments to state the real contract, and add a one-line "single-core,
IF-disabled" note at the page-table-edit sites and at the user-pointer validators
(`syscall/types.rs`). No behavior change.

### M2 — Kernel-space non-present faults silently demand-map fresh RW pages
`kernel/src/memory/vmm/page_fault.rs:124` backs *any* non-present higher-half kernel fault
with a fresh writable page (only `P=1`/reserved/OOM panic). A wild kernel pointer gets memory
instead of trapping, masking bugs and defeating kernel guard pages. **Fix:** restrict
auto-backing to the kernel heap arena (`HEAP_START_OFFSET` range, `heap/types.rs:41`); panic
on non-present kernel faults outside it. Coordinate with the heap-growth path so legitimate
growth still faults in.

### M3 — No physical-frame refcounting *(LOW — deprioritized by DD-1)*
> **Status after DD-1 (spawn, not fork):** the original CoW/fork rationale for this item is
> **removed**. This is now *optional cleanup*, not a prerequisite for any roadmap item. Do it
> only if the alias workaround becomes a maintenance burden, or later as an optimisation to
> share read-only code pages between multiple instances of the same binary. Left in the
> backlog for completeness; safe to defer indefinitely.

Shared frames (user-code aliased over kernel text) are freed via a manual boolean policy
(`release_user_code_pfns`, `memory/vmm/mapping.rs:576`) to avoid double-free. Works, doesn't
generalize. Note this aliasing exists independently of process creation, so `spawn` does
**not** remove it — the workaround stays either way; refcounting would merely make it less
fragile. **Optional fix:** add a per-frame refcount array in the PMM (`memory/pmm/`),
increment on alias/map, `release_pfn` only frees at zero. Separately (and still worth doing
under `spawn`): `destroy_user_address_space` currently reclaims only Code+Stack+Heap and
leaks any other user region (`mapping.rs:594`) — once `spawn`-created processes gain `mmap`
regions, extend teardown to reclaim them (this part does **not** need refcounting).

### M4 — Console holds an interrupt-disabling lock across full-screen VRAM flush
`with_console` (`kernel/src/console/interface.rs:121`) holds `GLOBAL_CONSOLE` (interrupt-
disabling `SpinLock`) for the whole closure; on the framebuffer path a scroll marks the full
screen dirty (`framebuffer.rs:350`) and `flush_to_vram` (`framebuffer.rs:295`)
`copy_nonoverlapping`s the entire framebuffer to slow VRAM with interrupts off — a multi-MB
MMIO copy per scroll in a critical section. **Fix:** copy the dirty region out under the
lock, then perform the VRAM blit *after* releasing the lock (or with interrupts re-enabled);
or narrow the dirty range on scroll (blit only the shifted region + the new bottom line).

### M5 — GPT `fallback_esp()` masks all failures as `Some(2048)`
`kernel/src/io/gpt.rs:19`/`:106` returns LBA 2048 on read failure, missing magic, or
ESP-not-found, so `main.rs:259`'s `.expect("ESP not found on GPT disk")` misreports genuine
errors. **Fix:** return `Result`/`Option` faithfully — `None` (or a typed error) when the GPT
is unreadable/absent, only use the 2048 heuristic for a clearly-valid-GPT-but-no-ESP-entry
case, and log which case fired.

### M6 — No per-syscall authorization
Every ring-3 task can call `Shutdown` (`syscall/dispatch/process.rs:299`), `Exec` arbitrary
binaries (`:247`), enumerate PCI/BIOS, and `Wait` on any task id (`:291`, no ownership
check). **Fix (incremental):** start with an authorization gate for `Shutdown` (and later
`Exec`), keyed off a task capability/privilege field added to the scheduler `TaskEntry`. This
is the seed of the capability model the driver-infrastructure plan already envisions.

### M7 — `yield_now` rings a spurious PIC EOI
`kernel/src/scheduler/roundrobin/mod.rs:550` fires `int 0x20`, flowing through `dispatch_irq`
→ PIC EOI (`arch/interrupts/mod.rs:212`) with no hardware IRQ in service; a non-specific EOI
could ack the wrong line if a real IRQ were mid-service. **Fix:** make `yield_now` call
`on_timer_tick(frame)` directly (the clean model the syscall `Yield` path already uses,
`handlers.rs:450`) instead of raising a software `int 0x20`.

### M8 — Duplicated syscall decode boilerplate + a wrong threshold
`kernel/src/syscall/user.rs` repeats the same ~5-line error-decode ladder ~40× (e.g.
`user_readline` lines 314–412). Worse, `sys_yield`'s decoder (`user.rs:60`) uses
`x >= SYSCALL_ERR_INVALID_ARG` which would misclassify an `Io`/`OutOfMemory` sentinel as
*success*. Also `decode_result` (`syscall/types.rs:353`) has a dead arm. **Fix:** one shared
`const fn decode(raw) -> Result<u64, SysError>` used by every wrapper; delete the dead arm;
unify thresholds. Keep it `const`/inline so the ring-3-aliased wrappers don't pull extra
higher-half pages (the original reason for the inlining — preserve that constraint).

### M9 — Kernel heap does not scrub freed blocks / zero on reuse
Freed kernel-heap blocks retain contents and `malloc` doesn't zero on reuse
(`memory/heap/types.rs`), so sensitive data lingers in the kernel heap (info-leak surface).
User frames are safe (re-zeroed on next fault-in). **Fix:** zero payloads on free (or on
reuse) in the kernel heap allocator; measure the perf cost and gate behind a config if
needed.

---

## 7. LOW findings / cleanups

- **L1** `main.rs` re-derives `&*(boot_info_raw as *const BootInfo)` via fresh `unsafe` ≥8×
  (lines 104, 109, 118, 178, 198, 238, 366, 389). Validate once, publish a `&'static
  BootInfo`, pass it around.
- **L2** `current_task_id()` returns a raw slot index, not a packed id, despite the name
  (`scheduler/roundrobin/api.rs:49`). Rename to `current_slot()` or return a packed id.
- **L3** Task-id generation truncation: `pack_task_id`/`task_id_generation` keep low 32 bits
  (`types.rs:105`,`:111`) while `api::task_generation` returns the full u64; equality compare
  in `wait.rs:102`/`:109` breaks after 2^32 spawns. Store the truncated generation in the
  slot too, or widen the packing.
- **L4** Keyboard F11/F12 (scancodes 0x57/0x58) decode to nothing (`drivers/keyboard.rs:482`);
  Pause/E1 unhandled. Map them.
- **L5** PCI comment bug: `drivers/pci/mod.rs:59` says offset `0x0C` but correctly reads
  `0x0E` for header type. Fix the comment.
- **L6** FAT32 hardcodes 512 everywhere instead of the parsed `bytes_per_sec`
  (`io/fat32.rs:9`), and `map_fat32_err` collapses `IsDirectory/BadChain/TooLarge/NotFat32`
  into `Io` (`fat32.rs:702`), losing error granularity. Truncated chains return a short
  buffer silently (`fat32.rs:271`).
- **L7** PMM/`allocator` arithmetic: unchecked `align_up`/`region_end` in PMM
  (`memory/pmm/types.rs:17`, `manager.rs:283`) and `frames_free` decrements unchecked
  (`manager.rs:303`,`:348`) — not currently reachable, but inconsistent with the checked
  arithmetic elsewhere. Add `debug_assert!`s or `checked_*`.
- **L8** `RingBuffer::clear()` (`sync/ringbuffer.rs:82`) is not safe against concurrent
  push/pop; document "reset-only." `ATTR_VOLUME_ID` entries aren't skipped in FAT32
  `read_file` (`fat32.rs:201`, dirs-only check). `disable/enable_blink_mode` are silent
  no-ops on framebuffer (`framebuffer.rs:597`).

---

## 8. Feature roadmap (do after C1/C2 and the HIGH items)

Ordered so each step unblocks the next; aligns with `docs/` plans and project memory
(BlockDevice/VFS → AHCI → framebuffer → UEFI → real HW are largely **done**).

1. **FAT32 write support + block/FAT cache.** Highest-leverage: the read path is solid, the
   cache is the biggest perf win (today every `next_cluster` re-reads a 512-byte sector,
   `fat32.rs:426`; whole files are read into RAM on open, `fat32.rs:569`), and writes
   (free-cluster search, dir-entry mutation, FAT-mirror writeback, `CACHE FLUSH`) are the
   biggest functional gap. Depends on H6 (ATA flush) and H1 (AHCI writes) for durability.
2. **Subdirectory traversal + path parser + LFN reads.** Turns the flat 8.3-root facade
   (`normalize_name`, `fat32.rs:482`; root-only walks at `:156`/`:285`) into a usable FS.
3. **Real VFS layer:** per-process fd table (move fds out of `Fat32FsState`, `fat32.rs:522`),
   a mount table, `stat`/metadata; lets ATA and AHCI volumes coexist and removes the leaky
   `print_root_directory`/`close_task_fds` from the `FileSystem` trait (`io/vfs.rs:60`,`:63`).
   Subsumes H4.
4. **`copy_from_user`/`copy_to_user` + exception-table fault fixups**, then enable **SMAP/
   SMEP** (CR4 bits 20/21, plus STAC/CLAC around user access). Closes C1 structurally and
   makes the syscall boundary trustworthy.
5. **AHCI DMA rework** (finishing H1): scatter-gather PRDT into the caller buffer, 48-bit
   LBA, multi-sector commands, then interrupt-driven completion (PxIE/GHC.IE) and eventually
   NCQ.
6. **`spawn`-based multiprocessing + a small ELF loader** *(per DD-1 — replaces the former
   "fork + CoW" step; no CoW, no frame refcounting)*. Add a `spawn(image, args)` syscall that
   builds a **fresh** address space and loads a new program (extend `process::exec_from_image`
   rather than clone a parent — the Windows `CreateProcess` / POSIX `posix_spawn` model), plus
   an **ELF loader** (`docs/todo_elf.md`) to replace the flat-binary format. This is what
   makes user space genuinely extensible (multiple concurrent programs). **Sub-steps:**
   (a) generalise `exec_from_image` into `spawn(image, argv)` returning a PID; (b) parse ELF
   program headers and map PT_LOAD segments with correct RWX/NX per segment (reuse
   `classify_user_region`/NX policy); (c) pass `argv`/`argc` on the new user stack per the ABI;
   (d) ensure each spawned process gets its own fd table — **depends on roadmap #3** (per-
   process fd table) for clean multi-process file handling, otherwise fds collide.
   **Explicitly out of scope (DD-1):** `fork()`, copy-on-write, and physical-frame refcounting
   (finding M3). If POSIX `fork` is ever wanted later (e.g. to port an existing Unix shell),
   *that* is when CoW + M3 refcounting + a write-fault CoW branch in the page-fault handler
   become prerequisites again — but `spawn` needs none of it.
7. **SMP (last).** Per-CPU state (GDT/TSS/IDT load, CR3, RSP0), APIC/IO-APIC instead of the
   8259 PIC, **TLB shootdown IPIs** (today `invlpg` is local-only,
   `memory/vmm/page_table.rs:325`), per-CPU runqueues, and a careful pass revisiting every
   single-core invariant flagged in this doc (C2, H3, M1, and the lost-wakeup/ABA notes in
   the scheduler). Priorities, timed sleep, and a timer wheel fit here.

**Suggested near-term thread:** `1 → 2 → 3` for the biggest visible capability gain
(a real, writable, hierarchical filesystem), with `4` in parallel to harden the existing
boundary. Defer `7` until the single-core invariants are documented and C1/C2 are closed.

---

## 9. Appendix — how this review was produced

Full read of `main.rs`, `lib.rs`, `boot_info.rs`, `panic.rs`, `logging.rs`, `testing.rs`,
`sync/spinlock.rs`, `syscall/user.rs`, plus the Cargo/linker/toolchain config and the
`tests/` inventory; and a subsystem-by-subsystem deep read of memory (PMM/VMM/heap),
scheduler/sync/process, syscall/arch, drivers, and io/console. Findings marked CRITICAL (C1,
C2) are argued from the source but should be **confirmed with the reproduction tests
described in each item before large refactors** — they are subtle enough to warrant a red
test first.
