# FAT12 User-Program Loader: Deep Technical Walkthrough

This document explains, in implementation-level detail, how the current Rust kernel loads a user-mode program from the FAT12 partition, maps it into a dedicated user address space, starts it as a schedulable ring-3 task, and reclaims all resources again when the task exits.

The walkthrough is intentionally grounded in the current codebase and refers to these modules:

- `src/repl.rs`
- `src/process/types.rs`
- `src/process/loader.rs`
- `src/io/fat12.rs`
- `src/memory/vmm.rs`
- `src/scheduler/roundrobin.rs`
- `src/syscall/dispatch.rs`
- `src/arch/interrupts.rs`

The loader currently targets flat `.bin` payloads with a fixed virtual entry address. There is no ELF parser in this phase, no relocation processing, and no dynamic linking. That limitation is deliberate: it keeps the execution model deterministic and makes ownership and rollback behavior easier to audit.

---

## 1) Architectural Context and Invariants

The process-loading path is split across three subsystems that each enforce a different class of invariants. The FAT12 layer is responsible for producing a correct byte payload for a short 8.3 filename and for rejecting malformed directory/FAT chains. The process loader translates that payload into a concrete address-space materialization, including validation against the configured executable window and controlled mapping permissions. Finally, the scheduler owns runtime lifecycle and final teardown once a user task has been spawned.

What matters operationally is that these layers are intentionally decoupled. The FAT12 code does not know anything about page tables or task metadata. The VMM does not know where bytes came from. The scheduler does not parse file formats. This separation is what allows rollback logic to be explicit rather than implicit: every transition between stages is represented by small typed contracts (`ExecError`, `LoadedProgram`, explicit CR3 handoff), and each stage can reverse only what it owns.

The implementation is currently built around a strict user virtual-memory contract. User code occupies a dedicated 2 MiB window, and user stack occupies a dedicated 1 MiB window near the top of canonical low-half user space, with a guard page below the stack. The loader never maps outside these windows. This constraint is enforced by `map_user_page` in `src/memory/vmm.rs`, so policy is centralized in one place.

---

## 2) End-to-End Runtime Story (`exec` in the REPL)

At runtime, the entire flow starts in the shell command handler in `src/repl.rs`. When the user enters `exec <file>`, the REPL calls `process::exec_from_fat12(file_name)`. A successful return yields a scheduler task ID, and the REPL immediately enters foreground wait mode by calling `scheduler::wait_for_task_exit(task_id)`. This is important behaviorally: the shell does not continue immediately, but cooperatively yields until the spawned task has terminated.

In other words, execution is currently “foreground process execution” from the shell perspective, even though the scheduler itself remains round-robin and preemptive/cooperative as designed.

The control-flow shape can be visualized as follows.

```text
User
  |
  | exec HELLO.BIN
  v
REPL (execute_command)
  |
  | process::exec_from_fat12("HELLO.BIN")
  v
Process loader
  |
  | load_program_image -> FAT12 read + validation
  | map_program_image_into_user_address_space
  | spawn_loaded_program
  v
Scheduler
  |
  | task_id returned
  v
REPL
  |
  | wait_for_task_exit(task_id)
  | (yield loop while task is alive)
  v
User task runs in ring 3
  |
  | Exit syscall
  v
Syscall dispatch -> zombie marking -> scheduler reap
  |
  v
REPL wait loop ends, prompt is shown again
```

---

## 3) FAT12 Stage: Turning `HELLO.BIN` into a Byte Vector

The first concrete operation in the process loader is `load_program_image(file_name_8_3)` in `src/process/loader.rs`. This function delegates to `fat12::read_file` and then applies process-level image-size validation.

The FAT12 side, implemented in `src/io/fat12.rs`, performs a full short-name lookup and cluster-chain traversal. The incoming shell token is normalized through `normalize_8_3_name` into canonical uppercased, space-padded 11-byte FAT short-name form. The root directory region is then scanned for a matching active entry. If the entry exists but has the directory attribute bit set, the loader receives `Fat12Error::IsDirectory`; if no entry matches, it receives `Fat12Error::NotFound`; malformed names fail earlier as `Fat12Error::InvalidFileName`.

Once a regular-file entry is found, FAT#1 is read and the file content is reconstructed by following the FAT12 12-bit cluster chain until exactly `file_size` bytes have been produced. Loop detection and invalid cluster values are treated as corruption (`CorruptFatChain`). This strictness is intentional because partially trusting a broken chain would create unpredictable copy behavior in the executable mapping stage.

The on-disk geometry assumptions are the standard 1.44 MiB FAT12 layout used in this project:

```text
LBA 0        : Reserved sector (boot)
LBA 1..9     : FAT #1
LBA 10..18   : FAT #2
LBA 19..32   : Root directory (224 entries -> 14 sectors)
LBA 33..end  : Data area (clusters)
```

From the process loader perspective, FAT12-specific errors are intentionally translated into process-domain errors via `map_fat12_error`, so callers above the loader do not have to reason about filesystem internals.

---

## 4) Process Contracts: Entry, Stack, and Image Limits

The process constants in `src/process/types.rs` define the ABI-like contract between loader, scheduler, and user binaries. `USER_PROGRAM_ENTRY_RIP` is anchored at `vmm::USER_CODE_BASE`, and `USER_PROGRAM_INITIAL_RSP` is anchored at `vmm::USER_STACK_TOP - 16` to preserve 16-byte stack alignment expectations. The maximum accepted image size is exactly `vmm::USER_CODE_SIZE`, exposed as `USER_PROGRAM_MAX_IMAGE_SIZE`.

This means the flat binary must fit entirely inside one fixed code window. The loader uses this as a hard acceptance criterion before touching PMM/VMM state. A too-large image is rejected early with `ExecError::FileTooLarge`, preventing partially created address spaces and keeping failure semantics simple.

The effective user virtual layout is:

```text
Higher addresses
    ^
    |
    | USER_STACK_TOP (exclusive) = 0x0000_7FFF_F000_0000
    | +-----------------------------------------------+
    | | User stack region (1 MiB)                     |
    | | [USER_STACK_BASE, USER_STACK_TOP)             |
    | +-----------------------------------------------+
    | +-----------------------------------------------+
    | | Guard page (4 KiB, intentionally unmapped)    |
    | +-----------------------------------------------+
    |
    |        (large unmapped gap)
    |
    | USER_CODE_BASE = 0x0000_7000_0000_0000
    | +-----------------------------------------------+
    | | User code region (2 MiB)                      |
    | | [USER_CODE_BASE, USER_CODE_END)               |
    | +-----------------------------------------------+
    |
    +-------------------------------------------------------> VA space
Lower addresses
```

---

## 5) Address-Space Materialization: The Mapping Transaction

The core function is `map_program_image_into_user_address_space(image: &[u8])` in `src/process/loader.rs`. Conceptually, this function executes a transaction with explicit rollback bookkeeping.

First, it validates size constraints again (defensive re-check). Then it creates a fresh root page table by calling `vmm::clone_kernel_pml4_for_user()`. The clone preserves shared kernel-half mappings but gives the process a distinct CR3 root. If this step fails, execution stops immediately with `ExecError::AddressSpaceCreateFailed`.

After a CR3 exists, the loader allocates all required code frames upfront and one bootstrap stack frame. Allocating everything before mapping is a deliberate policy choice: it prevents mixed “half mapped, then out of memory” states caused by late allocation failures in the middle of mapping loops.

The actual mapping and byte-copy work happens inside `vmm::with_address_space(user_cr3, || { ... })`, which temporarily switches CR3 to the target address space for the closure duration and restores the previous CR3 afterward. This is required because recursive page-table helper addresses in this VMM design always refer to the currently active hierarchy.

Inside that closure, the loader follows a two-phase permission model. In phase one, code pages are mapped writable so zero-fill and copy are possible. The stack bootstrap page at `USER_STACK_TOP - PAGE_SIZE` is also mapped writable. Then the entire mapped code range is zeroed and the FAT12 image bytes are copied to `USER_PROGRAM_ENTRY_RIP`. In phase two, code pages are remapped read-only (`writable = false`). That final permission state is an explicit policy boundary: loading is a privileged write operation, execution is not.

A successful transaction returns `LoadedProgram { cr3, entry_rip, user_rsp, image_len }`, which is exactly the descriptor needed by scheduler spawn logic.

The resulting mapping state can be imagined as:

```text
CR3 = process-specific user_cr3

USER_CODE_BASE
  +---------------------------+
  | code page 0  (U=1, W=0)   |
  +---------------------------+
  | code page 1  (U=1, W=0)   |
  +---------------------------+
  | ...                       |
  +---------------------------+

USER_STACK_TOP - 4096
  +---------------------------+
  | bootstrap stack (U=1,W=1) |
  +---------------------------+

USER_STACK_TOP (exclusive)
```

---

## 6) Rollback and Leak-Resistance on Failure Paths

The loader tracks exactly which resources have been allocated and which of them have actually been inserted into page tables. This distinction is critical. Mapped pages can be reclaimed through VMM teardown. Allocated-but-never-mapped frames are invisible to VMM and must be released explicitly through PMM.

On any mapping/copy failure, `cleanup_failed_program_mapping` runs. The first rollback step is `vmm::destroy_user_address_space_with_options(user_cr3, true)`, using the explicit “owned code PFNs” policy for loader-created images. After that, the loader iterates over allocation bookkeeping and releases residual PFNs that were never mapped (`code_pfns.skip(mapped_code_pages)` and optionally the stack PFN when stack mapping never happened).

This gives a robust two-layer rollback model:

1. VMM teardown reclaims mapped hierarchy and mapped leaves according to policy.
2. PMM cleanup reclaims allocation residue that never became part of any mapping.

That combination is what keeps the transaction leak-resistant even when failures happen in late setup steps.

---

## 7) Spawn Step and Ownership Transfer to Scheduler

`exec_from_fat12` completes by calling `spawn_loaded_program`, which in turn calls `scheduler::spawn_user_task_owning_code(entry_rip, user_rsp, cr3)`. The `_owning_code` variant is important because it sets the task’s teardown policy to release code PFNs on destruction. This matches loader semantics: these code pages are private process-owned frames, not aliases of shared kernel pages.

If spawn succeeds, ownership of CR3 and mapped leaves moves to scheduler/task lifecycle management. If spawn fails, the loader immediately destroys the address space with `destroy_user_address_space_with_options(..., true)` and returns `ExecError::SpawnFailed`.

This explicit ownership handoff is one of the central correctness properties of the design. At no point should CR3 ownership be ambiguous.

---

## 8) Teardown Policy: `owned` vs `alias` User Code Pages

The VMM exposes two teardown entry points:

- `destroy_user_address_space(pml4_phys)` (legacy default)
- `destroy_user_address_space_with_options(pml4_phys, release_user_code_pfns)`

The default path is alias-safe and keeps code PFNs reserved, because some paths may map user-visible aliases to kernel-owned pages. Loader-created binaries, however, are true owners of their code frames, so they must be destroyed with `release_user_code_pfns = true`.

Scheduler task metadata carries this policy in `release_user_code_pfns`, and `remove_task` applies it when reclaiming a user task. Stack pages are always treated as process-owned and are always released.

In practical terms, the cleanup decision matrix is:

```text
Task type / mapping origin         release_user_code_pfns
---------------------------------  -----------------------
Kernel task                        n/a (no user CR3 cleanup)
User task with code aliases        false
User task from FAT12 loader        true
```

---

## 9) Foreground `exec`, Exit Syscall, and Prompt Behavior

The current shell behavior is intentionally foreground-oriented. After spawning, REPL remains inside `wait_for_task_exit(task_id)` and repeatedly yields so the scheduler can run the child task. The prompt is shown only when liveness checks report that the task is gone.

When user code calls `Exit`, `syscall_exit_impl` marks the running task as `Zombie`. The syscall return path in `syscall_rust_dispatch` then directly calls `scheduler::on_timer_tick` for `Yield` and `Exit` syscalls, avoiding nested interrupt complexity and switching context using the current syscall frame. Zombie tasks are then reaped by scheduler logic once execution is safely off their stack.

So the lifecycle is not “instant free on exit,” but “mark-zombie now, reclaim on scheduler-safe point,” which is exactly the pattern required to avoid use-after-free on active kernel stacks.

---

## 10) User-Mode Program Structure in This Phase

A minimal user program in this system is a `#![no_std]`, `#![no_main]` binary with `_start` entry, explicit syscall wrappers, and panic-abort semantics. The current `hello` sample (`main64/user_programs/hello/src/main.rs`) writes a static message using `WriteConsole` and then exits via `Exit`. The syscall wrappers live in `main64/user_programs/common/syscall.rs` and use `int 0x80` with the kernel-defined register ABI.

Because the binary is linked at the same fixed virtual address as `USER_PROGRAM_ENTRY_RIP`, loader startup does not require relocation logic in this phase.

---

## 11) Build and FAT12 Image Integration

The standard Rust-kernel build scripts currently invoke `main64/build_user_programs.sh` to compile user payloads and produce `hello.bin`. During disk-image creation, both `main64/build_kernel_debug.sh` and `main64/build_kernel_release.sh` inject that payload into FAT12 as `HELLO.BIN` using `fat_imgen`.

That is why runtime invocation from REPL is simply:

```text
> exec HELLO.BIN
```

The chain from source to runnable file is therefore: Rust user crate -> flat binary (`objcopy`) -> FAT12 image entry -> runtime FAT12 lookup -> user mapping -> scheduler task.

---

## 12) Observability and Diagnostics

This path is instrumented enough to debug most failures from serial logs alone. VMM traces show page-fault and mapping behavior, PMM traces show frame ownership churn, heap traces show allocator activity, and syscall dispatch traces include syscall number/name/arguments/return values for every call.

A practical triage order for failed `exec` attempts is:

1. FAT12 lookup (`InvalidName`, `NotFound`, `IsDirectory`)
2. image size validation (`FileTooLarge`)
3. address-space creation (`AddressSpaceCreateFailed`)
4. mapping/copy path (`MappingFailed`)
5. scheduler spawn (`SpawnFailed`)
6. cleanup traces (to verify rollback completeness)

---

## 13) Test Coverage and Confidence Model

The current test suite validates the loader pipeline from several angles rather than with one monolithic test. `tests/fat12_test.rs` covers FAT12 parsing and file read behavior. `tests/process_contract_test.rs` validates process constants, image loading, and map/copy contracts. `tests/vmm_test.rs` exercises user mapping flags, clone/destroy behavior, and teardown policy edges. `tests/syscall_dispatch_test.rs` protects syscall IDs and dispatch behavior.

This layered coverage mirrors the layered architecture: storage, mapping, lifecycle, and syscall boundary each have dedicated assertions. For low-level kernel work, this is generally more maintainable than one giant end-to-end assertion because regressions are localized faster.

---

## 14) Current Limitations and Next Engineering Steps

This phase intentionally stops before full executable-format support and advanced process semantics. The most natural next upgrades are ELF segment loading with per-segment permissions, richer user-stack bootstrap (argv/envp), lazy code paging, and parent/child wait semantics with exit status transport. Another important hardening path is stricter TLB/permission-transition auditing for idempotent remap paths.

Even with those items pending, the existing implementation already provides a coherent and production-shaped pipeline: deterministic FAT12 load, explicit size and mapping contracts, controlled permission transitions, foreground shell semantics, and policy-aware teardown that distinguishes alias-safe from truly owned user code pages.
