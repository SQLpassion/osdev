# KAOS Kernel Code Review — `kernel/src` (2026-07-26)

> Full-source review of `main64/kernel/src/` (~22.7K lines across 89 files) at commit
> `a858067`. Every finding below was independently spot-checked against the current tree by
> re-reading the cited lines before being included — see §9 for exactly which claims were
> directly re-verified versus taken from subsystem-scoped sub-reviews. Where a claim could
> not be fully confirmed, that uncertainty is stated explicitly in the finding instead of
> being asserted as fact.

## Table of contents

1. [Overview](#1-overview)
2. [Architecture & code quality](#2-architecture--code-quality)
3. [Security & memory safety](#3-security--memory-safety)
   - [3.1 Unsafe-block audit](#31-unsafe-block-audit)
   - [3.2 Privilege boundaries & syscall input validation](#32-privilege-boundaries--syscall-input-validation)
   - [3.3 Interrupt/scheduler races, reentrancy, lock ordering](#33-interruptscheduler-races-reentrancy-lock-ordering)
   - [3.4 Integer/pointer arithmetic in memory management](#34-integerpointer-arithmetic-in-memory-management)
4. [Severity legend](#4-severity-legend)
5. [Findings index](#5-findings-index)
6. [Findings detail](#6-findings-detail)
7. [Feature roadmap](#7-feature-roadmap)
8. [Next features](#8-next-features)
9. [Appendix — methodology](#9-appendix--methodology)

---

## 1. Overview

KAOS is a mature `no_std` Rust x86_64 kernel: higher-half mapped, BIOS+UEFI boot, ring-3 user
space over an `int 0x80` syscall ABI, a round-robin scheduler with lazy FPU/SSE switching, a
4-level-paging VMM with recursive self-map, a segregated-free-list heap, AHCI+ATA+FAT32+GPT
storage, and PCI enumeration. The codebase is unusually well-documented for a hobby OS:
`SAFETY:` rationale accompanies essentially every `unsafe` block, and `docs/*.md` explains
each subsystem in tutorial depth.

**This review found no new memory-corruption-class bug reachable from ring 3.** The syscall
boundary's pointer-validation layer (`syscall/types.rs`) is applied consistently by every
handler that touches a userspace pointer. The two defects rated CRITICAL/HIGH below are a
boot-time diagnostic-path hazard and a test-framework robustness bug, not exploitable kernel
compromise. The bulk of the findings are MEDIUM/LOW: latent bugs not reachable via any live
call path today, lock-scope/perf issues, and documentation/duplication cleanups.

## 2. Architecture & code quality

### What's good

- **Deferred, two-phase teardown keeps slow work out of interrupt-disabling locks.**
  `scheduler/roundrobin/manager.rs:223-230` pushes a terminated task's CR3 onto
  `pending_free_address_spaces` instead of destroying the address space inline under `SCHED`;
  every `on_timer_tick` return path and `terminate_task` call `vmm::destroy_user_address_space`
  only after the lock guard has dropped. This is the H3 fix and it's applied consistently
  everywhere the pattern is needed, not just at the one call site the original bug was found in.
- **PMM frame refcounting replaced a fragile boolean workaround.**
  `memory/pmm/manager.rs:459-520` adds a real per-frame refcount array; `release_pfn` only
  frees at zero and saturates instead of wrapping specifically so a pathological caller can't
  wrap the count back down and trigger an early free. `destroy_user_address_space`
  (`memory/vmm/mapping.rs:608-717`) now also reclaims *any* user region via a generically
  bounded scan (`USER_ADDRESS_SPACE_SCAN_END`, `vmm_constants.rs:48`), not just Code/Stack/Heap.
- **The syscall pointer-validation layer is a genuine single source of truth.** Every dispatch
  handler across `syscall/dispatch/{bios,console,fs,pci,process}.rs` that accepts a ring-3
  pointer or length routes through `is_valid_user_buffer_readable`/`writable`
  (`syscall/types.rs:271-363`); no handler reimplements its own ad hoc bounds check. Traced
  end-to-end for this review across all five dispatch modules with no exception found.
- **`io::vfs`'s `with()` turned a documented UAF risk into a structural non-issue.** Rather
  than just documenting "don't call `mount`/`reset_mounted_fs` concurrently" (the originally
  planned H4 mitigation), `MOUNTED_FS: SpinLock<Option<Arc<dyn FileSystem>>>` (`io/vfs.rs:75`)
  clones the `Arc` under the lock before dropping the guard (`io/vfs.rs:110-127`) — `Arc`'s
  own guarantee (the allocation outlives the last clone) makes the dangling-pointer scenario
  impossible regardless of what `mount`/`reset_mounted_fs` do concurrently.
- **The panic handler is deliberately lock-free and heap-free.** `panic.rs:17-27` documents
  *why* (a panic can occur while `GLOBAL_CONSOLE`/`GLOBAL_SCREEN` is already held, and
  re-locking a single-core spinlock you already hold spins forever) and both the VGA
  (`drivers/screen.rs`'s `PanicScreenWriter`) and framebuffer paths honor this. Correct design
  for a last-resort diagnostic path — see §6 M1/H2 for where its *input assumptions* still
  have gaps.

### What's bad

- **The C2 fix ("scheduler critical section must run with `CR0.TS = 0`") is enforced at one
  of ~22 lock-entry points, not the choke-point almost everything else already funnels
  through.** `on_timer_tick` (`scheduler/roundrobin/mod.rs:301-317`) clears `TS` before taking
  `SCHED`; `handle_fpu_trap` clears it *after* taking the lock; every other entry —
  `mark_current_as_zombie`, `block_task`/`unblock_task`, `terminate_task`, `spawn_internal`,
  `init`/`start`, every `api.rs` accessor — goes through `with_scheduler`
  (`scheduler/roundrobin/mod.rs:147-150`), which never touches `CR0.TS`. See finding M2.
- **A doc-comment regression turned a shipped naming fix into a live logic bug.**
  `current_task_id()` (`scheduler/roundrobin/api.rs:49-59`) was changed to return a *packed*
  task id (slot + generation) instead of a bare slot index — but its own doc comment (lines
  49-54) still says the opposite ("the raw slot index... not a packed task identifier"), and
  a comparison three lines into `wait_for_task_exit` (`wait.rs:126`) was never updated to
  match, silently breaking the self-wait guard it implements. See finding M3.
- **The 4-level page-table walk (PML4→PDP→PD→PT, bail on non-present/huge at each level) is
  duplicated near-verbatim about 9 times**: `page_table.rs` (`pt_for_if_present:530`,
  `is_user_page_writable:577`, `is_user_page_readable:618`, `is_va_mapped:667`),
  `diagnostics.rs` (`:12`, `:63`), and `mapping.rs` (`:161`, `:741`, `:879`). Not a live bug —
  every copy currently agrees on huge-page handling — but a future correctness fix to how a
  huge page in the path is detected has to be hand-replicated across all nine sites, silently
  reintroducing the exact class of bug `HugePageInPath` (`mapping.rs:36-43`) exists to prevent.
- **`ahci.rs`'s highest-risk code (the DMA/FIS/PRDT construction path) is the one place in the
  crate where the project's own "`SAFETY:` on every `unsafe`" convention is dropped.** Every
  other driver (`ata.rs`, `keyboard.rs`, `serial.rs`, `pci/config.rs`) has per-block SAFETY
  rationale; `ahci.rs:637` and the entire `do_transfer` body (`ahci.rs:645-758`) have none. See
  finding M6.
- **Authorization is a single ad hoc boolean wired into exactly one syscall.**
  `TaskEntry.privileged` gates `Shutdown` cleanly (`syscall/dispatch/process.rs:306-319`,
  confirmed only the boot shell is ever spawned privileged) but `Exec`, `Wait`, and PCI/BIOS
  enumeration have no capability check at all — each future "this should be gated" decision
  needs its own bespoke wiring rather than a shared, table-driven check. See finding M9.

## 3. Security & memory safety

### 3.1 Unsafe-block audit

Roughly 520 `unsafe` occurrences across the crate were reviewed by subsystem. The overwhelming
majority carry a `SAFETY:`/`# Safety` justification that was checked against what the code
actually does and found **correct** — including every MSR/port-I/O helper (`arch/msr.rs`,
`arch/port.rs`), every ATA/keyboard/serial/PCI-config unsafe block, the FXSAVE/FXRSTOR/CR0.TS
helpers in `arch/fpu.rs`, the GDT/TSS singleton construction in `arch/gdt.rs`, the page-table
entry accessors in `memory/vmm/page_table.rs`, and the syscall-boundary pointer derefs in
`syscall/dispatch/*.rs` (all gated behind validation, see §3.2).

Two categories of exception were found, both restated as findings in §6:

- **Missing justification on the riskiest code, not incorrect justification.** `ahci.rs:637`
  (a `static mut ACTIVE_PORT` read) and the entire `do_transfer` DMA path (`ahci.rs:645-758`,
  the single largest and most privileged unsafe block in the crate — raw pointer derefs of
  `HbaPort`/`HbaCmdHeader`/`HbaCmdTbl`, physical-address translation, PRDT byte-count
  arithmetic) have **zero** SAFETY comment, unlike every comparable block elsewhere in the
  driver layer (**M6**). A smaller style-only gap of the same kind exists at
  `syscall/dispatch/fs.rs:22` (**L3**).
- **A documented unsafe-grade contract on a function that isn't `unsafe fn` and has no
  internal guard.** `memory/vmm/mapping.rs:785-797`'s `map_user_page` carries a `# Safety`
  section describing a real corruption hazard (recursive-mapping resolution racing a CR3
  switch) on a plain `pub fn`; both current call sites happen to satisfy the contract, but
  nothing stops a future caller from violating it undetected (**M7**).

No case was found where a `SAFETY:` comment's stated invariant was **false** given what the
guarded code does — the gaps above are missing/insufficient documentation and enforcement,
not incorrect claims.

### 3.2 Privilege boundaries & syscall input validation

Every syscall handler in `syscall/dispatch/{bios,console,fs,pci,process}.rs` that accepts a
ring-3 pointer, length, index, or fd was traced to its validation:

| Trust category | Validation found | Gap |
|---|---|---|
| Raw pointer + length (console write, framebuffer write, BIOS/PCI struct out-params) | `is_valid_user_buffer_readable`/`writable`, alignment checks, fixed-size clamps | None found |
| NUL-terminated string (`Exec`/`OpenFile`/`DeleteFile` name) | `read_user_string` validates every byte before dereference via the same buffer check | None found (one missing-comment style nit, **L3**) |
| Array index (BIOS memory-map entry, PCI device index) | Bounds-checked against the real count (`bib.memory_map_entries`, `Vec::get`) before use | None found |
| File descriptor | Ownership (`owner: Option<usize>` per open file) enforced **inside the FAT32 backend**, not the syscall/VFS layer | Implicit contract on whichever `FileSystem` impl is mounted; a future second backend that omits the check would silently reopen cross-task fd access (**L6**) |
| Task id (`Wait`) | Bit-masked slot/generation decode (no OOB/panic risk) | No ownership check — any task can `Wait` on any task id, a process-existence side channel (restates old M6, **M9**) |
| Capability (`Exec`, PCI/BIOS enumeration) | None | Any ring-3 task can spawn children or enumerate hardware (restates old M6, **M9**) |
| `SeekFile` offset | None beyond the buffer checks | Silently truncates `u64` to `u32` instead of rejecting `> u32::MAX` (**L4**) |

**No syscall handler was found that dereferences a ring-3-supplied pointer/length without
first routing through validation.** C1 remains structurally intact.

### 3.3 Interrupt/scheduler races, reentrancy, lock ordering

The kernel's load-bearing invariant — single core, `cli`-disabling spinlocks, `#PF`/IRQ paths
as interrupt gates — makes cross-lock ordering deadlocks structurally impossible today (only
one thread of control exists; two different locks can't deadlock without reentrant recursion).
The real risk class, per the fixed C2 bug, is an **unmasked exception re-entering a lock the
interrupted code already held**. This review re-audited that class specifically:

- `#NM` is the only exception verified to be masked by explicit design (`clear_ts()` before
  `SCHED` in `on_timer_tick`) — but that discipline is not universal (**M2**). Empirically,
  this is inert today: the kernel target (`x86_64-unknown-none`) disables SSE/MMX/AVX and
  forces soft-float, and disassembly of the built kernel binary shows zero incidental
  `xmm`/`ymm` instructions outside the four deliberate FXSAVE64/FXRSTOR64/FNINIT/LDMXCSR sites
  in `arch/fpu.rs`. It becomes live only if that target configuration ever changes.
- Kernel-mode `#PF`, `#GP`, `#UD`, `#DE` handlers were checked for scheduler-lock reentrancy:
  none call into `SCHED`-locked code from inside the handler (kernel-mode variants halt or
  return directly; user-mode variants call `mark_current_as_zombie()` then `on_timer_tick()`
  sequentially, never nested). No reentrancy found here.
- A logic bug (not a race) was found in the self-wait path: `wait_for_task_exit`'s guard for
  "task waiting on itself" never fires because of the packed-id/bare-slot mismatch above
  (**M3**) — this doesn't currently deadlock anything reachable from ring 3 (no syscall
  exposes a task's own packed id to pass back into `Wait`), but it is a real defect in a
  public scheduler API.
- `terminate_task` was found to leak file descriptors for the same packed-id/bare-slot reason
  (**M4**) — `exit_current_task` (the ring-3-reachable exit path) passes the id correctly;
  only `terminate_task` (currently only called from tests) gets it wrong.

### 3.4 Integer/pointer arithmetic in memory management

PMM region/frame arithmetic is now fully `checked_*`-based (a previously-applied fix,
confirmed at `memory/pmm/manager.rs:327-330,371-374,478,539` for region math and `:349,398,433,587`
for `frames_free` updates) and panics loudly on overflow rather than silently wrapping.
Heap block-splitting/coalescing (`memory/heap/types.rs`) was walked through worked examples for
the split-vs-consume decision, the tail-pointer repair on backward-merge, and
`grow_heap`'s boundary addition — no new overflow/underflow found beyond one latent gap:

- `mark_range_used` (`memory/pmm/manager.rs:323-352`) decrements `frames_free` unconditionally
  per bit, unlike its sibling `mark_frame_used` (`:366-403`), which explicitly checks
  free→used transition before decrementing so it's safe to call on an already-reserved frame.
  Not reachable via any current call site (the two `PhysicalMemoryManager::new()` call sites
  use documented-disjoint ranges) but a real hygiene gap in the same class L7 just fixed
  elsewhere (**M8**).

Also newly reviewed and confirmed **CRITICAL**: on-disk GPT structures are trusted past what
their own field validation guarantees — see **H1** in §6, which is a data-integrity/parsing
bug, not a heap/PMM arithmetic one, but belongs in this category by nature (an unchecked
value used directly as a slice-index stride).

## 4. Severity legend

| Level | Meaning |
|-------|---------|
| **CRITICAL** | User/media-triggerable kernel compromise or memory corruption, or a boot/runtime path that can fail with **zero** diagnostic output on the supported target. |
| **HIGH** | Reachable crash/DoS/hang (with usable diagnostics), or a bug in a load-bearing diagnostic/test path that hides real failures. |
| **MEDIUM** | Real defect, but not currently reachable via any live call path, or a lock-scope/latency/perf issue, or a robustness gap under corrupted-but-plausible input. |
| **LOW** | Cosmetic, documentation, dead code, or small hygiene/duplication cleanup. |

## 5. Findings index

| ID | Sev | Area | One-liner |
|----|-----|------|-----------|
| C1 | CRITICAL | panic/boot | Panic handler trusts the framebuffer as writable before it's mapped on a BIOS+VBE boot — can triple-fault with zero diagnostic output |
| H1 | HIGH | io/gpt | `parse_gpt_entries_sector` panics (OOB slice) for any GPT `SizeOfPartitionEntry` divisor of 512 below 128 |
| H2 | HIGH | testing | Test-framework panic handler drops the assertion message for every formatted panic (i.e. every failed `test_assert!`) |
| M1 | MEDIUM | main.rs | Early magic-check dereference has no upper bound despite its comment claiming one |
| M2 | MEDIUM | scheduler | C2's "`TS=0` before `SCHED`" guarantee is enforced at 1 of ~22 lock-entry points |
| M3 | MEDIUM | scheduler | `wait_for_task_exit`'s self-wait guard never fires (packed-id vs. bare-slot comparison bug) |
| M4 | MEDIUM | scheduler | `terminate_task` leaks a terminated task's open file descriptors |
| M5 | MEDIUM | drivers/ahci | `do_transfer` holds the interrupt-disabling lock across ~6M busy-poll iterations, not just register programming |
| M6 | MEDIUM | drivers/ahci | Highest-risk unsafe blocks (DMA/FIS/PRDT path) have no `SAFETY:` justification |
| M7 | MEDIUM | drivers/block | `AhciBlockDevice` doesn't clamp to AHCI's 48-bit LBA limit, unlike `AtaBlockDevice` |
| M8 | MEDIUM | memory/vmm | `map_user_page` documents an unsafe-grade precondition but is a safe `pub fn` with no internal guard |
| M9 | MEDIUM | memory/pmm | `mark_range_used` isn't idempotent against overlapping ranges, unlike `mark_frame_used` |
| M10 | MEDIUM | syscall | `Exec`/`Wait`/PCI/BIOS enumeration still have no per-syscall authorization (Shutdown fixed) |
| M11 | MEDIUM | logging | `print_captured_target` reads the capture buffer after releasing the logger lock |
| L1 | LOW | memory/vmm | Redundant page-table walk in `reclaim_user_range` (perf only) |
| L2 | LOW | memory/vmm | 4-level page-table walk duplicated ~9× across page_table.rs/diagnostics.rs/mapping.rs |
| L3 | LOW | syscall | Missing `SAFETY:` comment on `ptr.add(len)` in `fs.rs` |
| L4 | LOW | syscall | `SeekFile` silently truncates a `u64` offset to `u32` |
| L5 | LOW | syscall | Dead match arm in `decode_result` (test-only code path) |
| L6 | LOW | io/vfs | Fd-ownership enforcement lives only in the FS backend, not the VFS facade |
| L7 | LOW | io/fat32 | `print_root_directory` doesn't skip `ATTR_VOLUME_ID` entries, unlike the fixed `read_file` |
| L8 | LOW | drivers/ahci | `read_sectors`/`write_sectors` have no self-contained input validation (defense-in-depth) |
| L9 | LOW | io/fat32 | `map_fat32_err` still collapses several distinct errors into generic `Io` |
| L10 | LOW | io/fat32 | Sector size still hardcoded to 512 in most places instead of `self.bytes_per_sec` |
| L11 | LOW | io/vfs | `reset_mounted_fs` still ungated in production (now a logical foot-gun only, not a UAF) |
| L12 | LOW | console | Residual full-screen RAM-to-RAM copy still runs under `GLOBAL_CONSOLE`'s lock after the M4 fix |
| L13 | LOW | main.rs | Blanket `#![allow(dead_code)]` hides two genuinely unused functions |
| L14 | LOW | console/panic | `BootInfo` re-derivation duplicated outside the canonical `BootInfo::get()` accessor |
| L15 | LOW | console | Blink-mode no-op on the framebuffer backend is undocumented at the trait level |
| L16 | LOW | testing | `panic_message_contains` test helper can miss a substring spanning two `write_str` chunks |
| L17 | LOW | main.rs | PAT MSR write's `SAFETY:` comment states the wrong justification |
| L18 | LOW | keyboard | Pause/0xE1 scancode sequence still unhandled |

## 6. Findings detail

### C1 — CRITICAL: Panic handler can triple-fault (or hang with zero output) before the framebuffer is mapped, on a BIOS+VBE boot

**Files:** `kernel/src/panic.rs:48-69`, `kernel/src/main.rs:100-188`.

`BOOT_INFO_PTR` is published at `main.rs:106`, immediately after the magic-number check
succeeds — **before** `gdt::init()`, `fpu::init()`, `pmm::init()`, `interrupts::init()`,
`vmm::init()`, `heap::init()`, or `map_framebuffer()` run (lines 126-188). `BootInfo.video_type`
is set by the *bootloader*, not the kernel, so on any BIOS+VBE boot it already reads
`Framebuffer` at the moment `BOOT_INFO_PTR` is published.

`PanicFramebufferWriter::from_boot_info()` (`panic.rs:48-69`) treats `bi.fb_info.base_address`
as an "identity-mapped" writable pointer as soon as `video_type == Framebuffer` — with no
check for whether the framebuffer has actually been mapped yet. The kernel's own comment at
`main.rs:119-122` states the hazard directly:

```rust
// NOTE: Do NOT touch the linear framebuffer here. On a BIOS/VBE boot it lives at a high
// physical address that the bootstrap loader's identity map (low 16 MiB) does not cover,
// and no page-fault/IDT handler exists yet — a write would fault and triple-fault the CPU.
// The framebuffer is mapped and painted later, once the VMM is up (see `map_framebuffer`).
```

**Failure scenario:** Any panic between `main.rs:106` and `main.rs:188` — e.g. from
`pmm::init(true)` encountering a malformed memory map, or any other fallible early-init call —
enters `panic()`, which picks the framebuffer branch and calls `fb.clear(bg)`
(`panic.rs:72-86`), writing through `self.base.add(...)` to the unmapped physical address.
Before `interrupts::init()` (line 159) this has no `#PF` handler at all → immediate triple
fault, per the kernel's own comment. After `interrupts::init()` but still before
`map_framebuffer()`, the exact outcome depends on VMM page-fault behavior not fully re-verified
in this pass — plausibly a fault loop / recursive panic, since the same broken writer would be
invoked again. Either way: the earliest, most fault-prone part of boot (PMM/interrupt/VMM/heap
init) is exactly the window where the panic handler cannot reliably produce output, defeating
the diagnostic path's entire purpose.

**Fix:** gate the framebuffer panic writer on an explicit "mapped" flag set at the end of
`map_framebuffer()`:

```rust
// boot_info.rs
pub static FRAMEBUFFER_MAPPED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
```
```rust
// main.rs, end of map_framebuffer()
crate::boot_info::FRAMEBUFFER_MAPPED.store(true, core::sync::atomic::Ordering::Release);
```
```rust
// panic.rs
fn from_boot_info() -> Option<Self> {
    if !crate::boot_info::FRAMEBUFFER_MAPPED.load(Ordering::Relaxed) {
        return None; // not yet mapped — fall back to the VGA-text panic writer
    }
    let raw = BOOT_INFO_PTR.load(Ordering::Relaxed);
    ...
}
```
The VGA-text fallback (`drivers::screen::PanicScreenWriter`) writes to `0xB8000`, which is
always within the low identity map, so it remains safe this early even on a boot with no VGA
text plane active — it just won't be visible on a graphics-only display in that narrow window,
which is a strictly better outcome than a silent triple fault.

---

### H1 — HIGH: `parse_gpt_entries_sector` panics (out-of-bounds slice) for a GPT with `SizeOfPartitionEntry < 128`

**File:** `kernel/src/io/gpt.rs:86` (`parse_gpt_header`), `:99-101` (`parse_gpt_entries_sector`).

```rust
// gpt.rs:86
if entry_size == 0 || entry_size > 512 || 512 % entry_size != 0 {
    return None;
}
```
accepts any of `{1,2,4,8,16,32,64,128,256,512}`. But entries are read at fixed offsets inside
each entry's stride regardless of `entry_size`:
```rust
// gpt.rs:99-108
for i in 0..entries_in_this_sector {
    let offset = (i * entry_size) as usize;
    let guid = &entry_sector[offset..offset + 16];              // <-- panics here for small entry_size
    if guid == ESP_TYPE_GUID {
        let start_lba = u64::from_le_bytes(
            entry_sector[offset + 0x20..offset + 0x28].try_into().unwrap(),
        );
        ...
```
**Verified directly:** for `entry_size = 8`, `entries_per_sector = 512/8 = 64`; a standard GPT
declares `num_entries = 128 ≥ entries_per_sector`, so `entries_in_this_sector` for the first
sector is `min(64, 128) = 64`. At loop index `i = 63`, `offset = 504`, and
`entry_sector[504..520]` is sliced against a `[u8; 512]` array — `520 > 512` — an out-of-bounds
slice that panics **unconditionally**, before even reaching the GUID comparison. This is
reached on the very first entry sector scanned, not a corner case.

**Failure scenario:** A disk (real, or a crafted/corrupted image passed to QEMU — e.g. a torn
write to the GPT header) with a valid `"EFI PART"` signature, `NumberOfPartitionEntries = 128`,
but `SizeOfPartitionEntry ∈ {1,2,4,8,16,32,64}` (any of these divide 512 and are accepted).
`main.rs:250` calls `io::gpt::find_esp_start_lba().expect(...)` on the UEFI boot path — this
panics the kernel during boot, before any user code runs, from nothing more than an
attacker-controlled or corrupted disk header (no ring-3 access required). Confirmed not
covered by the existing test suite (`tests/gpt_test.rs` only exercises `entry_size ∈ {0, 123,
128}`, never a valid-but-small divisor).

**Fix:**
```rust
// gpt.rs, inside parse_gpt_header — UEFI mandates SizeOfPartitionEntry >= 128, which also
// guarantees every field this driver reads (GUID at 0x00, StartingLBA at 0x20..0x28) fits
// inside one entry's stride, so parse_gpt_entries_sector can never index past the sector.
if entry_size < 128 || entry_size > 512 || 512 % entry_size != 0 {
    return None;
}
```
As defense-in-depth, also bounds-check before slicing in `parse_gpt_entries_sector`:
```rust
for i in 0..entries_in_this_sector {
    let offset = (i * entry_size) as usize;
    if offset + 0x28 > entry_sector.len() {
        break; // corrupt/undersized entry_size — stop instead of panicking
    }
    ...
```

---

### H2 — HIGH: Test-framework panic handler drops the assertion message for every formatted panic

**File:** `kernel/src/testing.rs:100-102`.

```rust
if let Some(message) = info.message().as_str() {
    debugln!("  Message: {}", message);
}
```
`PanicMessage::as_str()` returns `Some` only for a panic built from a bare string literal with
no format arguments. Every `test_assert_eq!`/`test_assert!` failure (`testing.rs:128-161`)
constructs its `panic!` with interpolated arguments (`"... {:?} ...", left, right`), so
`as_str()` returns `None` and the message-printing branch is silently skipped for essentially
every real test failure this framework exists to report. Note `panic.rs:142` (the production
panic path) already does this correctly via `Display` (`writeln!(writer, "Message: {}",
info.message())`) — confirmed by direct comparison, so the fix is exactly to mirror it.

**Failure scenario:** A test does `test_assert_eq!(result, 42)` and fails with `result == 7`.
Serial/QEMU output shows `[FAILED]`, file:line, and the summary counts, but never the actual
`left`/`right` values — anyone debugging a CI failure from the serial log alone has to
re-run locally or read the source to guess what failed.

**Fix:**
```rust
// testing.rs:100-102 — PanicMessage always implements Display; no Option needed.
debugln!("  Message: {}", info.message());
```

---

### M1 — MEDIUM: Early `BootInfo` magic-check dereference has no upper bound

**File:** `kernel/src/main.rs:100-112`.

```rust
if boot_info_raw > 0x1000 && boot_info_raw.is_multiple_of(8) {
    // SAFETY:
    // - `boot_info_raw` is non-null, aligned, and within low memory space.
    let magic = unsafe { *(boot_info_raw as *const u64) };
```
The comment claims the address is "within low memory space," but only a lower bound and
8-byte alignment are actually checked — there is no upper bound tying this to the documented
low-16-MiB identity map the surrounding comment (`main.rs:90-93`) says this check exists to
respect.

**Failure scenario:** Any 8-byte-aligned value `> 0x1000` reaches the raw dereference. A
future or misbehaving legacy loader that passes a large `kernel_size` (page/sector-aligned
values are the common case) interpreted as this raw pointer would dereference an address with
no guarantee of being mapped, before `interrupts::init()` has installed a `#PF` handler —
triple-faulting with no diagnostic, the exact failure mode the surrounding comment says this
check is meant to prevent.

**Fix:**
```rust
const LOW_MEM_IDENTITY_MAP_LIMIT: u64 = 0x0100_0000; // 16 MiB, matches the loader's identity map
if boot_info_raw > 0x1000 && boot_info_raw < LOW_MEM_IDENTITY_MAP_LIMIT && boot_info_raw.is_multiple_of(8) {
```

---

### M2 — MEDIUM: The C2 "`TS=0` before `SCHED`" invariant is enforced at one lock-entry point, not all of them

**File:** `kernel/src/scheduler/roundrobin/mod.rs:147-150` (`with_scheduler`), contrast
`:301-317` (`on_timer_tick`) and `:479-488` (`handle_fpu_trap`).

```rust
// mod.rs:147-150 — used by ~20 call sites (block_task, unblock_task, terminate_task,
// spawn_internal, init, start, every api.rs accessor) and never touches CR0.TS.
pub(super) fn with_scheduler<R>(f: impl FnOnce(&mut SchedulerMetadata) -> R) -> R {
    let mut sched = SCHED.lock();
    f(&mut sched)
}
```
`on_timer_tick` clears `TS` before taking `SCHED` (verified, `mod.rs:317`); `handle_fpu_trap`
clears it *after* taking the lock. Every other path that mutates `SchedulerMetadata` goes
through `with_scheduler`, which does neither.

**Currently inert, verified two ways:** (1) the kernel's target (`kernel/.cargo/config.toml`
targets `x86_64-unknown-none`, whose built-in spec disables SSE/MMX/AVX and forces
soft-float), and (2) disassembly of the built kernel shows zero incidental `xmm`/`ymm`
instructions outside the four deliberate FXSAVE64/FXRSTOR64/FNINIT/LDMXCSR sites in
`arch/fpu.rs`. So no compiler-emitted instruction can trigger `#NM` inside a `with_scheduler`
call today. **This is not a structural property of the code, though** — it depends entirely on
the current target configuration never changing (e.g. re-enabling SSE for float performance,
or a future dependency shipping SIMD-optimized code), and `tests/scheduler_nm_deadlock_test.rs`
only exercises the `on_timer_tick` path, so a regression here would ship silently.

**Fix:** centralize the guarantee in the one choke-point almost everything already uses:
```rust
pub(super) fn with_scheduler<R>(f: impl FnOnce(&mut SchedulerMetadata) -> R) -> R {
    // SAFETY: mirrors on_timer_tick's C2 rationale — TS must be 0 before any SCHED-protected
    // critical section so it stays provably #NM-free regardless of target SSE configuration.
    unsafe { fpu::clear_ts() };
    let mut sched = SCHED.lock();
    f(&mut sched)
}
```
and reorder `handle_fpu_trap` to clear `TS` before `SCHED.lock()` instead of after.

---

### M3 — MEDIUM: `wait_for_task_exit`'s self-wait guard never fires (packed-id vs. bare-slot bug)

**Files:** `kernel/src/scheduler/roundrobin/wait.rs:103-129`, root cause
`kernel/src/scheduler/roundrobin/api.rs:49-59`.

```rust
// wait.rs:104, 122-126
let slot = task_id_slot(task_id);        // bare slot of the *target*
...
if let Some(waiter_task_id) = current_task_id() {   // PACKED id of the caller
    if waiter_task_id == slot {          // <-- always false: packed id has generation bits set
```
```rust
// api.rs:55-59 — confirmed: this DOES return a packed id today...
pub fn current_task_id() -> Option<usize> {
    with_scheduler(|meta| {
        meta.running_slot.map(|slot| super::types::pack_task_id(slot, meta.slots[slot].generation))
    })
}
```
```rust
// api.rs:49-54 — ...but the doc comment was never updated and says the opposite:
/// This is the raw slot index used internally by the scheduler. It is not a
/// packed task identifier.
```
Traced via `git log -p` to the commit that changed `current_task_id()`'s implementation from a
bare slot to a packed id (for the earlier "L2" naming fix) — the doc comment and this one
comparison were not updated to match.

**Failure scenario:** if any code path calls `wait_for_task_exit(own_task_id)` (a task waiting
on itself), the intended self-wait fast path never triggers; the comparison falls through to
the blocking-queue path, which finds the condition true (the caller is alive), registers on
`TASK_EXIT_WAITQUEUE`, and blocks the caller — permanently, since only `terminate_task`/
`remove_task` ever wake that queue for a given task, and neither runs on a task that is stuck
waiting on itself. **Not reachable via the current ring-3 syscall ABI** (no syscall exposes a
task's own packed id to pass back into `Wait`) — this is a defect in `scheduler::
wait_for_task_exit` as a public internal API, not a live production bug.

**Fix:**
```rust
// wait.rs:126
if task_id_slot(waiter_task_id) == slot {
```
and correct `api.rs:49-54`'s doc comment to describe the packed-id behavior it actually has.

---

### M4 — MEDIUM: `terminate_task` leaks a terminated task's open file descriptors

**File:** `kernel/src/scheduler/roundrobin/wait.rs:63-65`, contrast
`kernel/src/scheduler/roundrobin/mod.rs:582-585` (`exit_current_task`, which gets this right).

```rust
pub fn terminate_task(task_id: usize) -> bool {
    let slot = task_id_slot(task_id);
    crate::io::vfs::close_task_fds(slot);   // passes the BARE slot
```
`Fat32OpenFile::owner` is populated via `owner: crate::scheduler::current_task_id()`
(`io/fat32.rs:638`) — a **packed** id — and `close_task_fds`'s retain predicate compares
against whatever is passed in (`fat32.rs:754`). `exit_current_task` passes the packed id
straight through and works correctly; `terminate_task` extracts just the slot first, so its
retain predicate never matches any of the terminated task's actual open files.

**Failure scenario:** a task with open files is forcibly terminated via
`scheduler::terminate_task` (rather than exiting itself via the `Exit` syscall) — its
`Fat32OpenFile` entries (including the cached whole-file `Vec<u8>` contents, since FAT32 reads
files fully into RAM) are never removed, leaking the descriptor and its backing memory.
**Currently not reachable in production** — `terminate_task` has no call site outside
`kernel/tests/*.rs` today; it is presumably intended for a future forced-kill syscall.

**Fix:**
```rust
pub fn terminate_task(task_id: usize) -> bool {
    let slot = task_id_slot(task_id);
    crate::io::vfs::close_task_fds(task_id);   // pass the packed id, matching owner's format
```

---

### M5 — MEDIUM: AHCI `do_transfer` holds the interrupt-disabling lock across the entire completion poll

**File:** `kernel/src/drivers/ahci.rs:629-761`.

```rust
fn do_transfer(...) -> Result<(), AhciError> {
    let _guard = AHCI_LOCK.lock();          // disables interrupts; held for the WHOLE function
    ...
    let mut timeout = 1_000_000;             // free-slot wait, still under _guard
    loop { ... }
    ...
    let mut timeout2 = 5_000_000;            // completion wait, still under _guard
    loop { ... }
}
```
`SpinLock::lock()` disables interrupts on entry and restores them only when the guard drops —
so every AHCI read/write disables IRQs, the timer tick, and keyboard input for the full
duration of both busy-poll loops (up to 6,000,000 iterations combined), not just the register
programming in between. Contrast with `ata.rs`'s `poll_status_until`
(`drivers/ata.rs:342-400`), which samples status under a short-lived lock and cooperatively
yields between checks.

**Failure scenario:** any FAT32 file read/write routed through `AhciBlockDevice` (any UEFI
boot) blocks all other tasks and all interrupt-driven I/O for as long as the underlying disk
command takes to complete — worse on slower real SATA hardware than in QEMU.

**Fix (sketch):** release the lock before the completion-poll loop, re-acquiring only for the
brief register snapshot:
```rust
drop(_guard);
loop {
    let (ci, is) = { let _g = AHCI_LOCK.lock(); (read_volatile(&p.ci), read_volatile(&p.is)) };
    if (ci & (1 << slot)) == 0 { break; }
    if (is & (1 << 30)) != 0 { return Err(AhciError::PortError); }
    core::hint::spin_loop(); // or scheduler::yield_now(), mirroring ata.rs's pattern
}
```

---

### M6 — MEDIUM: AHCI's highest-risk unsafe blocks have no `SAFETY:` justification

**File:** `kernel/src/drivers/ahci.rs:637`, `:645-758`.

```rust
let port = unsafe { ACTIVE_PORT.ok_or(AhciError::NotInitialized)? };  // no SAFETY comment
...
unsafe {
    let p = &mut *port;   // start of a 110-line unsafe block: raw HbaPort/HbaCmdHeader/
    ...                   // HbaCmdTbl derefs, virt_to_phys translation, PRDT arithmetic —
}                          // zero SAFETY comment for any of it
```
This is the single largest and most privileged unsafe block in the crate and the only one
found in this review with no stated rationale at all, unlike every comparable block in
`ata.rs`/`keyboard.rs`/`serial.rs`/`pci/config.rs`. Not a functional defect by itself (the
logic was independently checked and appears correct — serialized by `AHCI_LOCK`, addresses
derived from driver-programmed `clb`/`ctba` values within the allocated per-port frame), but
it is the one place in the codebase where a future editor has no documented invariants to
check a change against.

**Fix:** add SAFETY comments capturing what's actually relied upon:
```rust
// SAFETY:
// - `ACTIVE_PORT` is written exactly once, in `init_ports`, before the scheduler or any
//   other task can call read/write_sectors; after boot it is read-only. Reads here happen
//   under AHCI_LOCK, serializing against any future writer.
```
```rust
// SAFETY:
// - `port` was validated non-null and points at a live, UC-mapped HbaPort inside the ABAR
//   MMIO region during init_ports/port_rebase.
// - AHCI_LOCK (held by _guard) serializes all command-slot 0 access.
// - cmd_header/cmd_tbl/fis are derived from clb/ctba addresses this driver itself programmed
//   and identity-mapped; they stay within the single allocated 4KB per-port frame.
```

---

### M7 — MEDIUM: `AhciBlockDevice` doesn't clamp to AHCI's 48-bit LBA limit

**File:** `kernel/src/drivers/block.rs:96,108`.

```rust
chunked(lba, count, u64::MAX, |chunk_lba, chunk_cnt, off| { ... ahci::read_sectors(...) })
```
uses `u64::MAX` as the max addressable LBA, so `chunked`'s range check never rejects an LBA
beyond what AHCI's FIS can actually encode (48 bits, `ahci.rs:728-735`). `AtaBlockDevice`
correctly clamps to `ATA_MAX_LBA = 0x0FFF_FFFF` for comparison.

**Failure scenario:** a corrupted or crafted GPT partition entry's `StartingLBA` (read as a
raw, unclamped `u64` at `gpt.rs:105-109`) that exceeds 2^48 would silently truncate inside the
FIS instead of being rejected — every subsequent read/write built from it addresses the
wrong, wrapped physical sector rather than failing.

**Fix:**
```rust
const AHCI_MAX_LBA: u64 = 0xFFFF_FFFF_FFFF; // 48-bit LBA, READ/WRITE DMA EXT
// pass AHCI_MAX_LBA instead of u64::MAX to chunked() in both read/write methods
```

---

### M8 — MEDIUM: `map_user_page` documents an unsafe-grade precondition on a safe `pub fn` with no internal guard

**File:** `kernel/src/memory/vmm/mapping.rs:785-797`.

```rust
/// # Safety
/// ... requires a stable active address space while it runs ...
/// Callers must execute it only inside `with_address_space` ... that:
/// - disables interrupts for the full duration, and
/// - guarantees `CR3` does not change until the function returns.
/// If this precondition is violated, ... can race and write into the wrong page-table hierarchy.
pub fn map_user_page(virtual_address: u64, pfn: u64, writable: bool) -> Result<(), MapError> {
```
Uses the `# Safety` rustdoc convention Rust reserves for functions the compiler can't check,
on a function that is not `unsafe fn`. Both current call sites
(`process/loader.rs:180`, `syscall/dispatch/process.rs:182`) correctly wrap the call in
`with_address_space(...)`, but nothing in the type system enforces this for a future caller.

**Fix:** either mark it `unsafe fn` (consistent with `switch_page_directory`,
`mapping.rs:470`), or add a cheap debug-only guard:
```rust
pub fn map_user_page(virtual_address: u64, pfn: u64, writable: bool) -> Result<(), MapError> {
    debug_assert!(
        !crate::arch::interrupts::are_enabled(),
        "map_user_page: must run with interrupts disabled (inside with_address_space)"
    );
    ...
```

---

### M9 — MEDIUM: `mark_range_used` isn't idempotent against overlapping ranges

**File:** `kernel/src/memory/pmm/manager.rs:323-352`.

```rust
fn mark_range_used(&mut self, range_start: u64, range_end: u64) {
    ...
    for bit in first_bit..end_bit {
        unsafe { set_bit(bit, bitmap) };
        r.frames_free = r.frames_free.checked_sub(1).unwrap();   // always decrements
    }
}
```
Its sibling `mark_frame_used` (`manager.rs:366-403`) explicitly checks the bit before
decrementing so it's safe to call on an already-reserved frame; `mark_range_used` has no such
guard.

**Failure scenario:** not reachable today — `PhysicalMemoryManager::new()` calls it once (BIOS
path) or twice with two ranges documented as disjoint (UEFI path). If a future change (or a
bootloader supplying a `pmm_metadata_base` landing inside `[KERNEL_OFFSET, STACK_TOP)`) ever
causes these ranges to overlap, every frame in the overlap silently double-decrements
`frames_free` — wrong accounting immediately, and a hard panic (`checked_sub().unwrap()`) once
enough double-decrements underflow it.

**Fix:** mirror `mark_frame_used`'s idempotency:
```rust
fn mark_range_used(&mut self, range_start: u64, range_end: u64) {
    for r in self.regions().iter_mut() {
        let region_end = r.start.checked_add(r.frames_total.checked_mul(PAGE_SIZE).unwrap()).unwrap();
        let overlap_start = range_start.max(r.start);
        let overlap_end = range_end.min(region_end);
        if overlap_start >= overlap_end { continue; }
        let first_bit = (overlap_start - r.start) / PAGE_SIZE;
        let end_bit = (overlap_end - r.start) / PAGE_SIZE;
        let bitmap = r.bitmap_start as *mut u64;
        for bit in first_bit..end_bit {
            let (word, mask) = ((bit / 64) as usize, 1u64 << (bit % 64));
            if unsafe { *bitmap.add(word) } & mask != 0 { continue; } // already used
            unsafe { set_bit(bit, bitmap) };
            r.frames_free = r.frames_free.checked_sub(1).unwrap();
        }
    }
}
```

---

### M10 — MEDIUM: `Exec`/`Wait`/PCI/BIOS enumeration still have no per-syscall authorization

**Files:** `kernel/src/syscall/dispatch/process.rs:247-296`, `dispatch/pci.rs:11-15`,
`dispatch/bios.rs:11-18`.

Restates a still-open authorization gap identified previously. `Shutdown` is now correctly gated
by `TaskEntry.privileged` (verified: only the boot shell is ever spawned privileged); `Exec`,
`Wait`, and hardware enumeration remain open to every ring-3 task.

**Failure scenario:** a malicious ring-3 task loops `Exec("SOME.BIN")`, spawning unbounded
unprivileged children until scheduler slots/PMM frames exhaust (DoS); or loops `Wait(task_id)`
over a range of ids to fingerprint which task ids are currently alive (existence side-channel).
Since `Exec`-spawned tasks are always unprivileged, the practical risk today is resource
exhaustion and information leakage, not privilege escalation.

**Fix (stopgap, smaller than the planned capability-gated-driver-syscalls work in
`docs/todo_drivers.md`):** extend the mechanism already proven for `Shutdown` into a small
counter/bitmask rather than building a full capability system:
```rust
// scheduler/roundrobin/types.rs
pub struct TaskEntry { ..., pub exec_count: u32 }

// syscall/dispatch/process.rs
pub fn syscall_exec_impl(name_ptr: *const u8) -> SyscallResult<u64> {
    let task_id = scheduler::current_task_id().ok_or(SyscallError::InvalidArg)?;
    if !scheduler::try_increment_exec_count(task_id, MAX_CHILD_EXECS) {
        return Err(SyscallError::PermissionDenied);
    }
    ...
}
```

---

### M11 — MEDIUM: `logging::print_captured_target` reads the capture buffer after releasing the logger lock

**File:** `kernel/src/logging.rs:127-172`.

```rust
let (ptr, len, overflow) = with_logger(|state| {
    (state.capture_buf.as_ptr(), state.capture_len, state.capture_overflow)
});
// lock released here
...
let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };  // read happens later, unlocked
```
`ptr`/`len` are snapshotted under `LOGGER.inner`'s lock, but the lock is released before the
slice is built and formatted to the screen — a slow, multi-line operation. There is a real
caller pattern for this: `memory/vmm/mod.rs` enables capture, lets ordinary logging append to
the buffer during normal operation, then dumps it later without disabling capture first.

**Failure scenario:** a debug/REPL command dumps captured VMM debug output while capture is
still enabled; mid-dump, a timer interrupt preempts to another task that logs through the same
buffer, potentially appending past the snapshotted `len` (harmless, bounded) or — if something
concurrently resets `capture_len` — starting to overwrite `capture_buf` from offset 0 while the
dump is still reading those same offsets. Bounded (buffer never reallocated, no OOB), but can
garble/truncate the debug output or make the UTF-8 decode fail and silently return nothing.

**Fix:** hold `LOGGER.inner` for the entire read-and-format loop, or copy the full snapshot
into a local buffer while still under the lock before formatting it.

---

### L1–L18 — LOW findings

Grouped for brevity; each is cleanup/hygiene/documentation, not a live defect.

| ID | File:line | Finding | Fix |
|----|-----------|---------|-----|
| L1 | `memory/vmm/mapping.rs:741-778` | `reclaim_user_range` resolves the full page-table path per page, then its callee re-resolves it again — ~2× the walks needed for a full teardown scan | Have the present-page case walk one PD/PT at a time instead of re-entering per page |
| L2 | `memory/vmm/page_table.rs:530,577,618,667`; `diagnostics.rs:12,63`; `mapping.rs:161,741,879` | 4-level page-table walk duplicated ~9× | Extract a shared `fn walk_levels(va) -> WalkResult` enum-matched by each call site |
| L3 | `syscall/dispatch/fs.rs:22` | `unsafe { ptr.add(len) }` has no `SAFETY:` comment (verified sound: `len` only grows after the previous address was validated) | Add the comment stating why overflow can't occur |
| L4 | `syscall/dispatch/fs.rs:102-105` | `SeekFile`'s `u64` offset is silently truncated to `u32` instead of rejected | `if offset > u32::MAX as u64 { return Err(SyscallError::InvalidArg); }` |
| L5 | `syscall/types.rs:421` | Dead match arm in `decode_result`, confirmed test-only (not on any live syscall path — `syscall::user::decode` is used instead) | Delete the arm |
| L6 | `io/fat32.rs:653-755` vs `syscall/dispatch/fs.rs` | Fd-ownership check lives only in the FAT32 backend, not the VFS facade — implicit contract for any future second backend | Document the requirement on the `FileSystem` trait in `io/vfs.rs` |
| L7 | `io/fat32.rs:335-338` | `print_root_directory` doesn't skip `ATTR_VOLUME_ID` (0x08) entries, unlike the fixed `read_file` (`fat32.rs:195-199`) | `if attr == 0x0F \|\| attr == 0x08 { continue; }` |
| L8 | `drivers/ahci.rs:615-627,692-695` | `read_sectors`/`write_sectors` have no self-contained `sector_count==0` guard or graceful PRDT-overflow handling (`assert!` panics instead); not reachable via current callers since `block.rs` always guards both | Mirror `ata.rs`'s guards; replace the `assert!` with a returned `AhciError` |
| L9 | `io/fat32.rs:759-767` | `map_fat32_err` still collapses `NotFat32`/`IsDirectory`/`BadChain`/`TooLarge` into generic `FsError::Io` | Add matching `FsError` variants |
| L10 | `io/fat32.rs` (multiple) | Sector size still hardcoded to 512 in most places instead of `self.bytes_per_sec` (harmless only because `mount()` rejects non-512 volumes) | Use `self.bytes_per_sec as usize` consistently |
| L11 | `io/vfs.rs:209-211` | `reset_mounted_fs` has no compile/runtime gate; the original UAF risk is now moot (Arc-based `with()`), but it remains a logical foot-gun if called concurrently with an in-flight `with()` | Document the residual caveat explicitly |
| L12 | `console/framebuffer.rs:297-353,366-405` | The RAM-to-RAM backbuffer→scratch copy still runs inside `with_console`'s lock after the M4 fix moved only the VRAM write outside it; a scroll marks the whole screen dirty | Narrow the scroll's dirty range to the shifted region + new bottom line |
| L13 | `main.rs:8,423-439` | Blanket `#![allow(dead_code)]` hides two functions (`kernel_va_to_phys`, `kernel_va_to_user_code_va`) with zero callers anywhere in `kernel/src`/`kernel/tests` | Remove if genuinely unused, or scope the `allow` to specific items |
| L14 | `console/framebuffer.rs:62-68`, `panic.rs:48-55` | Both re-derive `&BootInfo` from `BOOT_INFO_PTR` instead of calling the canonical `BootInfo::get()` (which the main L1 fix centralized elsewhere); `framebuffer.rs:67`'s comment ("Checked pointer") overstates what's actually checked (non-null only, not the magic) | Call `boot_info::BootInfo::get()` at both sites |
| L15 | `console/interface.rs:87-91`, `framebuffer.rs:645-651` | `disable/enable_blink_mode` are silent no-ops on the framebuffer backend; the trait doc describes the contract purely in VGA terms | Note explicitly in the trait doc that this is VGA-specific and a no-op elsewhere |
| L16 | `testing.rs:163-192` | `panic_message_contains`'s `Write` impl checks each formatted chunk independently, so a search string spanning two chunks is missed | Accumulate into a local buffer and search once over the full text |
| L17 | `main.rs:381-389` | PAT MSR write's SAFETY comment justifies via "ring 0" when the real invariant is that only one mapping (`memory/vmm/mapping.rs:301`) ever selects PAT1 | Update the comment to state the actual invariant |
| L18 | `drivers/keyboard.rs` | Pause/0xE1 scancode sequence still unhandled (F11/F12 were already fixed) | Map the 0xE1 prefix sequence |

## 7. Feature roadmap

> Carried forward from an earlier planning pass and updated with current status where this
> review found something already done. Ordered so each step unblocks the next; aligns with
> `docs/` plans and project memory (BlockDevice/VFS → AHCI → framebuffer → UEFI → real HW are
> largely **done**).

**Recorded design decision:** process creation uses `spawn(image, args)`, not `fork()`+`exec()`.
New processes get a **fresh** address space and load a new program image (the Windows
`CreateProcess` / POSIX `posix_spawn` model), extending the existing `process::exec_from_image`
machinery. Consequence: copy-on-write is out of scope — its only real driver would have been
`fork()`, and with `spawn` there is no parent address space to share. Physical-frame
refcounting was originally treated as optional cleanup under this decision; it has since been
**implemented anyway** (§2, "PMM frame refcounting replaced a fragile boolean workaround"),
independent of the fork/CoW question, because it also closed a generic teardown-reclaim gap.
If POSIX `fork` is ever wanted later (e.g. to port an existing Unix shell), that is when CoW
and a write-fault CoW branch in the page-fault handler would become prerequisites again —
`spawn` needs neither.

1. **FAT32 write support + block/FAT cache.** Highest-leverage: the read path is solid, the
   cache is the biggest perf win (today every `next_cluster` re-reads a 512-byte sector,
   `fat32.rs:426`; whole files are read into RAM on open, `fat32.rs:569`), and writes
   (free-cluster search, dir-entry mutation, FAT-mirror writeback, `CACHE FLUSH`) are the
   biggest functional gap. Depends on ATA cache-flush and AHCI write support for durability —
   both already implemented.
2. **Subdirectory traversal + path parser + LFN reads.** Turns the flat 8.3-root facade
   (`normalize_name`, `fat32.rs:482`; root-only walks at `:156`/`:285`) into a usable FS.
3. **Real VFS layer:** per-process fd table (move fds out of `Fat32FsState`, `fat32.rs:522`),
   a mount table, `stat`/metadata; lets ATA and AHCI volumes coexist and removes the leaky
   `print_root_directory`/`close_task_fds` from the `FileSystem` trait (`io/vfs.rs:60`,`:63`).
4. **`copy_from_user`/`copy_to_user` + exception-table fault fixups**, then enable **SMAP/
   SMEP** (CR4 bits 20/21, plus STAC/CLAC around user access). The manual validation layer
   already makes the syscall boundary safe today (§3.2) — this step replaces runtime checks
   with a compiler/CPU-enforced boundary, which is more robust long-term and removes the last
   hand-maintained validation logic from the trusted computing base.
5. **AHCI DMA rework — largely done.** Scatter-gather PRDT into the caller buffer, 48-bit LBA,
   and multi-sector commands are already implemented (confirmed in this review). Remaining:
   interrupt-driven completion (PxIE/GHC.IE) and eventually NCQ, plus the lock-scope and
   LBA-clamping fixes from §6 (**M5**, **M7**).
6. **`spawn`-based multiprocessing + a small ELF loader.** Add a `spawn(image, args)` syscall
   that builds a fresh address space and loads a new program (extend `process::exec_from_image`
   rather than clone a parent), plus an **ELF loader** (`docs/todo_elf.md`) to replace the
   flat-binary format. This is what makes user space genuinely extensible (multiple concurrent
   programs). **Sub-steps:** (a) generalise `exec_from_image` into `spawn(image, argv)`
   returning a PID; (b) parse ELF program headers and map `PT_LOAD` segments with correct
   RWX/NX per segment (reuse `classify_user_region`/NX policy); (c) pass `argv`/`argc` on the
   new user stack per the ABI; (d) ensure each spawned process gets its own fd table — depends
   on step 3 (per-process fd table) for clean multi-process file handling, otherwise fds
   collide. **Explicitly out of scope:** `fork()` and copy-on-write (see the design decision
   above).
7. **SMP (last).** Per-CPU state (GDT/TSS/IDT load, CR3, RSP0), APIC/IO-APIC instead of the
   8259 PIC, **TLB shootdown IPIs** (today `invlpg` is local-only,
   `memory/vmm/page_table.rs:325`), per-CPU runqueues, and a careful pass revisiting every
   single-core invariant flagged in this document — the `CR0.TS`/`SCHED` lock-entry gap
   (**M2**), the VMM lock's actual synchronization scope (§3.3), the address-space-teardown
   lock ordering, and the lost-wakeup/ABA notes in the scheduler. Priorities, timed sleep, and
   a timer wheel fit here.

**Suggested near-term thread:** `1 → 2 → 3` for the biggest visible capability gain (a real,
writable, hierarchical filesystem), with `4` in parallel to harden the existing syscall
boundary. Defer `7` until the single-core invariants above are fully documented.

## 8. Next features

Every subsystem's review converged on the same conclusion: the forward-looking gaps that
genuinely matter are already captured by the roadmap in §7 above (and by
`docs/todo_elf.md`/`docs/todo_drivers.md`/`docs/todo_uefi_kernel_pagetables.md`). This review
does not propose reordering that roadmap. One small, narrowly-scoped addition emerged directly
from a defect found in §6:

- **A debug/test-only PMM invariant self-check.** A function that recomputes, per region,
  `frames_total - frames_free` against a direct popcount of the bitmap, and asserts they
  match. This would have caught **M9** immediately in CI (a double-decrement without a
  matching bit flip diverges the two counts) and is cheap given `regions_snapshot()`
  (`memory/pmm/manager.rs:73-84`) already exists for read-only diagnostic access. Not a general
  "add more tests" suggestion — a targeted invariant check for the one accounting class that
  `mark_range_used`/`mark_frame_used`/`alloc_frame`/`release_pfn` all touch independently.

## 9. Appendix — methodology

This review was produced by five subsystem-scoped passes (memory/PMM/VMM/heap;
scheduler/interrupts/FPU/GDT/sync; syscall/process; drivers/io/vfs; console/boot/arch-misc),
each reading every file in its scope in full rather than sampling. Every subsystem pass began
by re-verifying previously-known findings in its area against the current tree.

For this synthesis, the following specific claims were **independently re-derived by directly
reading the cited source** (not merely trusted from a sub-review): the GPT `entry_size`
out-of-bounds slice arithmetic (H1, worked through by hand for `entry_size=8, i=63`); the
`BOOT_INFO_PTR` publish ordering relative to `interrupts::init`/`map_framebuffer` and the
explicit unmapped-framebuffer code comment (C1); the `testing.rs` message-drop code (H2); the
`with_scheduler`/`on_timer_tick` TS-clear asymmetry and the kernel's soft-float target
configuration (M2); the `wait_for_task_exit` self-wait comparison and `current_task_id`'s
packed-id return (M3); the `terminate_task` bare-slot fd-close call (M4); and the AHCI
`do_transfer` lock scope across both poll loops (M5). Findings not on this list were taken
from the relevant subsystem pass at the stated confidence level; where a sub-review itself
flagged uncertainty (e.g. the exact CPU-level outcome in C1's post-`interrupts::init` window,
or M9's real-world reachability), that hedge was preserved rather than resolved into a firmer
claim than the evidence supports.
