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

---

## Priority 4 — LOW

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
4. **R-04, R-09, R-10, R-11, R-12** (remaining MEDIUMs, independent of each other).
6. **R-13 … R-23** (LOWs, each independent).
7. Record observations O-1…O-7 in docs/CLAUDE.md where relevant.
