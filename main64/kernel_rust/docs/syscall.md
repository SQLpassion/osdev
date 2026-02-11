# Syscalls in This Kernel: A Deep Technical Tutorial (`int 0x80`)

This tutorial explains the current syscall path in this Rust kernel from first principles.
It is written for programmers who are new to OS internals, but it stays close to real implementation details.

Covered source files:

- `src/syscall/mod.rs`
- `src/syscall/types.rs`
- `src/syscall/dispatch.rs`
- `src/syscall/abi.rs`
- `src/syscall/user.rs`
- `src/arch/interrupts.rs`
- `src/arch/interrupts_stubs.rs`
- `src/main.rs`
- `src/scheduler/roundrobin.rs`
- `src/arch/gdt.rs`

---

## 1) What a syscall is (in plain terms)

A syscall is a controlled transition from user mode (ring 3) into kernel mode (ring 0).

Why this exists:

- User code must not directly access privileged CPU/memory/IO operations.
- The kernel exposes a narrow API surface (syscalls) to perform privileged actions safely.

In this kernel, the transition mechanism is `int 0x80`.

---

## 2) The layers in this codebase

Current syscall implementation is split into layers:

1. `types.rs`
- syscall numbers (`SyscallId`)
- raw return/error constants (`SYSCALL_ERR_*`, `SYSCALL_OK`)
- `decode_result` helper

2. `abi.rs`
- raw ABI emitters (`syscall0`, `syscall1`, `syscall2`)
- writes registers and executes `int 0x80`

3. `user.rs`
- ergonomic wrappers (`sys_yield`, `sys_write_serial`, `sys_exit`)
- converts raw return values to typed `Result`

4. `dispatch.rs`
- kernel-side switchboard (`dispatch`)
- routes syscall number -> concrete kernel implementation

5. `mod.rs`
- assembly of the module, reexports API, compatibility aliases

The compatibility path `syscall::arch::syscall_raw::*` points to the same raw ABI functions in `abi.rs`.

---

## 3) Syscall ABI contract (register-level)

When user/kernel code invokes this syscall path, the ABI is:

Input:
- `RAX` = syscall number
- `RDI` = arg0
- `RSI` = arg1
- `RDX` = arg2
- `R10` = arg3

Output:
- `RAX` = raw return value

Implemented syscall IDs (`types.rs`):

- `Yield = 0`
- `WriteSerial = 1`
- `Exit = 2`

Raw return code space:

- `SYSCALL_OK = 0`
- `SYSCALL_ERR_UNSUPPORTED = u64::MAX`
- `SYSCALL_ERR_INVALID_ARG = u64::MAX - 1`

`user.rs` decodes these raw values into:
- `Ok(value)`
- `Err(SysError::Enosys)`
- `Err(SysError::Einval)`
- `Err(SysError::Unknown(code))`

---

## 4) IDT setup for `int 0x80`

The syscall vector is configured in `arch/interrupts.rs`:

- vector: `SYSCALL_INT80_VECTOR = 0x80`
- handler: `int80_syscall_stub`
- gate setup: `set_handler_with_dpl(..., 3)`

Key points:

- The gate is present.
- It is an interrupt gate (`IDT_INTERRUPT_GATE`).
- DPL is 3, so ring-3 code is allowed to invoke it.

Without DPL=3, `int 0x80` from user mode would fault.

---

## 5) End-to-end control flow overview

### 5.1 Bird's-eye sequence

```text
Ring 3 task
  |
  |  int 0x80
  v
CPU consults IDT[0x80]
  |
  v
int80_syscall_stub (assembly)
  |
  v
syscall_rust_dispatch(frame)
  |
  v
syscall::dispatch(syscall_nr, args)
  |
  +--> Yield       -> scheduler::yield_now()
  +--> WriteSerial -> serial write
  +--> Exit        -> scheduler::exit_current_task()
  |
  v
return raw result in frame->rax
  |
  v
stub restores regs, iretq
  |
  v
Back to caller context (or task gone for Exit)
```

### 5.2 Data path

```text
Input registers before int 0x80:
  RAX = nr, RDI = a0, RSI = a1, RDX = a2, R10 = a3

SavedRegisters frame:
  frame.rax, frame.rdi, frame.rsi, frame.rdx, frame.r10

Rust dispatch:
  result = dispatch(frame.rax, frame.rdi, frame.rsi, frame.rdx, frame.r10)
  frame.rax = result

Output register after iretq:
  RAX = result
```

---

## 6) Assembly entry (`int80_syscall_stub`) in detail

Location: `arch/interrupts_stubs.rs`

Stub operations:

1. `cli`
- enters with interrupts disabled for a predictable critical section.

2. Push general-purpose registers
- creates a memory image matching `SavedRegisters` layout.

3. Pass `rsp` (saved-register block pointer) as argument to Rust dispatch
- `RDI = rsp`

4. Align stack and call `syscall_rust_dispatch`
- preserves ABI expectations for Rust function call.

5. Receive potentially updated frame pointer in `RAX`
- move back to `RSP`

6. Pop registers in reverse order

7. `iretq`
- returns to interrupt origin context.

### 6.1 Stack shape during syscall handling

```text
Higher addresses

+----------------------------------+
| CPU-pushed return frame          |
| (RIP, CS, RFLAGS, RSP, SS)       |
+----------------------------------+
| pushed GPRs (SavedRegisters)     |
| r15 ... rax                      |
+----------------------------------+  <- RSP passed to Rust dispatch

Lower addresses
```

Note: this kernel models return state with `InterruptStackFrame` in `interrupts.rs` and uses that contract consistently with scheduler/IRQ paths.

---

## 7) Rust bridge: `syscall_rust_dispatch`

Location: `arch/interrupts.rs`

Responsibilities:

- interpret `*mut SavedRegisters` from assembly
- extract syscall number and args from saved registers
- call `crate::syscall::dispatch(...)`
- write result to `frame.rax`
- return frame pointer to assembly stub

Why this split is useful:

- Assembly stays tiny and mechanical.
- Policy/logic remains in Rust.
- ABI boundary is explicit and auditable.

---

## 8) Kernel dispatcher behavior (`dispatch.rs`)

`dispatch(...)` is the kernel-side syscall switchboard.

### 8.1 `Yield`

- Path: `SyscallId::Yield` -> `scheduler::yield_now()`
- Return: `SYSCALL_OK`

`yield_now()` triggers software interrupt on the timer vector path so scheduler can choose another runnable task.

### 8.2 `WriteSerial(ptr, len)`

- `len == 0` -> return `0`
- null `ptr` with non-zero len -> `SYSCALL_ERR_INVALID_ARG`
- else read bytes from caller buffer and send to COM1
- return written byte count

### 8.3 `Exit(exit_code)`

- calls `scheduler::exit_current_task()`
- scheduler tears down current task and reschedules

Unknown syscall number:
- return `SYSCALL_ERR_UNSUPPORTED`

---

## 9) User-facing wrappers (`user.rs`)

`user.rs` makes syscall invocation easier for callers.

- `sys_yield()`
  - uses `abi::syscall0(Yield)`
  - returns `Result<(), SysError>`

- `sys_write_serial(buf)`
  - uses `abi::syscall2(WriteSerial, ptr, len)`
  - returns `Result<usize, SysError>`

- `sys_exit(code) -> !`
  - uses `abi::syscall1(Exit, code)`
  - expected to never return; has terminal fallback loop

The wrapper-local decoder (`decode_syscall_result`) translates raw codes to typed errors.

---

## 10) Concrete walkthrough: `userdemo` in `main.rs`

This is the easiest way to understand the entire path in practice.

Command:
- REPL command `userdemo`

Function:
- `run_user_mode_serial_demo`

### 10.1 Setup phase

1. `map_userdemo_task_pages()`
- maps required code pages for the demo call chain
- maps one user message page
- maps one user stack page

2. `write_userdemo_message_page()`
- writes `[ring3] hello from user mode via int 0x80\n` into mapped user page

3. compute entry RIP for ring-3 task
- start from kernel VA of `userdemo_ring3_task`
- translate into user-code alias VA

4. spawn user task
- `spawn_user_task(entry_rip, USER_SERIAL_TASK_STACK_TOP, cr3)`

### 10.2 Task body phase

`userdemo_ring3_task` does exactly:

- syscall `WriteSerial`
- syscall `Exit`

using:
- `syscall::arch::syscall_raw::syscall2(...)`
- `syscall::arch::syscall_raw::syscall1(...)`

Then REPL task waits until the task slot disappears.

### 10.3 Sequence diagram for this exact demo

```text
REPL task (kernel)                      userdemo task (ring3)
-------------------                     ---------------------
spawn_user_task(...)  ---------------->  scheduled via iretq
                                         syscall2(WriteSerial)
                                         int 0x80
                                         -> IDT[0x80]
                                         -> int80_syscall_stub
                                         -> syscall_rust_dispatch
                                         -> dispatch(WriteSerial)
                                         <- result in RAX
                                         <- iretq to ring3
                                         syscall1(Exit, 0)
                                         int 0x80
                                         -> dispatch(Exit)
                                         -> exit_current_task()
(wait task_frame_ptr is None) <------    task removed by scheduler
```

---

## 11) How scheduler + GDT/TSS support syscalls from ring 3

For a user task, scheduler builds an initial `InterruptStackFrame` with:

- `cs = USER_CODE_SELECTOR`
- `ss = USER_DATA_SELECTOR`
- `rip = entry_rip`
- `rsp = user_rsp`
- `rflags = 0x202` (IF enabled)

Before resuming a user task, scheduler updates `TSS.RSP0`:

- `gdt::set_kernel_rsp0(task.kernel_rsp_top)`

Why this matters:

- on ring3 -> ring0 events (such as `int 0x80`), CPU needs a valid ring-0 stack target.
- `TSS.RSP0` provides that kernel stack top.

ASCII relationship:

```text
User task selected
   |
   +--> scheduler programs TSS.RSP0 = task.kernel_rsp_top
   |
   +--> iretq enters ring3
           |
           +--> int 0x80
                    |
                    +--> CPU uses ring0 stack (from TSS.RSP0)
                    +--> enters stub/dispatch on kernel stack
```

---

## 12) Current bootstrap limitations (important)

The `userdemo` path is intentionally transitional:

- It aliases required kernel text pages into user VA window.
- It currently uses the active/shared CR3 path in demo startup.

This is useful for validating ring3 transitions and syscall plumbing quickly.
It is not the final process model.

Target end-state:

- true loader-provided user binaries (e.g. ELF text/data segments)
- per-process address spaces (cloned CR3 roots)
- no kernel-text alias dependency for user program code

---

## 13) Practical debugging checklist

If `int 0x80` fails from ring 3, check in this order:

1. IDT gate for vector `0x80`
- present, correct handler, DPL=3

2. Stub/struct layout agreement
- push/pop order matches `SavedRegisters`
- frame pointer handoff is correct

3. User selectors/frame
- user task has valid `CS/SS` ring-3 selectors
- `RIP/RSP` mapped and canonical

4. TSS
- `ltr` executed during GDT init
- `TSS.RSP0` set before running user task

5. mappings
- code pages executable/readable in user view
- stack page mapped writable

6. return code handling
- raw result in `RAX` interpreted via decoder

---

## 14) Quick source map

- Syscall module entry: `src/syscall/mod.rs`
- Syscall constants/types: `src/syscall/types.rs`
- Kernel dispatcher: `src/syscall/dispatch.rs`
- Raw ABI wrappers: `src/syscall/abi.rs`
- User wrappers: `src/syscall/user.rs`
- Interrupt subsystem: `src/arch/interrupts.rs`
- Interrupt/syscall stubs: `src/arch/interrupts_stubs.rs`
- User demo entry/setup: `src/main.rs`
- Task frame + context switch behavior: `src/scheduler/roundrobin.rs`
- GDT/TSS + `RSP0`: `src/arch/gdt.rs`
