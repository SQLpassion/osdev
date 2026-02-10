# Preemptive Multitasking via Timer-Driven Context Switching

This document provides an in-depth technical explanation of how the KAOS kernel
implements preemptive multitasking.  It traces the complete lifecycle of a
context switch — from hardware timer firing through register save, scheduler
decision, register restore, and resumption of a different task — and explains
why each step is necessary.

## Table of Contents

1. [The Core Idea](#1-the-core-idea)
2. [Hardware Foundation: PIT and PIC](#2-hardware-foundation-pit-and-pic)
3. [The IDT and Interrupt Gate Contract](#3-the-idt-and-interrupt-gate-contract)
4. [The IRQ Assembly Stub — Where the Magic Happens](#4-the-irq-assembly-stub--where-the-magic-happens)
5. [The Rust Dispatch Layer](#5-the-rust-dispatch-layer)
6. [Scheduler Core: `on_timer_tick`](#6-scheduler-core-on_timer_tick)
7. [Returning a Different Frame Pointer — The Actual Context Switch](#7-returning-a-different-frame-pointer--the-actual-context-switch)
8. [Synthetic Task Frames: Making `iretq` Boot a New Task](#8-synthetic-task-frames-making-iretq-boot-a-new-task)
9. [Stack Memory Layout Per Task](#9-stack-memory-layout-per-task)
10. [Synchronization: Why Interrupts Must Be Disabled](#10-synchronization-why-interrupts-must-be-disabled)
11. [The Bootstrap Frame: Getting Back to the Kernel REPL](#11-the-bootstrap-frame-getting-back-to-the-kernel-repl)
12. [Complete Walkthrough: First Three Timer Ticks](#12-complete-walkthrough-first-three-timer-ticks)
13. [Source File Map](#13-source-file-map)

---

## 1. The Core Idea

A running program does not voluntarily give up the CPU.  The kernel forces it
by configuring a hardware timer (the Programmable Interval Timer, PIT) to fire
an interrupt at a fixed frequency.  Every time this interrupt fires, the CPU
involuntarily suspends whatever code was executing, saves just enough state to
return later, and jumps to a kernel-defined handler.

The insight that makes context switching possible is:

> **The interrupt handler does not have to return to the code that was
> interrupted.  It can return to a completely different task — as long as it
> restores that other task's saved register state before executing `iretq`.**

This is the entire mechanism.  Everything else — the assembly stubs, the
scheduler data structures, the spinlock — exists to implement this one trick
safely and correctly.

---

## 2. Hardware Foundation: PIT and PIC

### Programmable Interval Timer (PIT)

The PIT is an Intel 8253/8254-compatible chip that generates periodic
interrupts.  It has an internal oscillator running at **1,193,182 Hz**.  We
program it with a *divisor* to produce interrupts at a desired frequency:

```
frequency = 1,193,182 / divisor
```

The kernel programs the PIT to fire at **250 Hz** (one interrupt every ~4 ms):

```rust
// interrupts.rs — init_periodic_timer(250)
let divisor = 1_193_182 / 250;   // = 4772 (0x12A4)
cmd.write(0x36);                  // Channel 0, rate generator mode
data.write((divisor & 0xFF) as u8);  // Low byte
data.write((divisor >> 8) as u8);    // High byte
```

Every 4 ms, the PIT asserts the **IRQ0** line on the Programmable Interrupt
Controller.

### Programmable Interrupt Controller (PIC)

The two cascaded 8259A PICs translate hardware IRQ lines into CPU interrupt
vectors.  During kernel init, the PICs are remapped so that:

| IRQ Line | CPU Vector | Source            |
|----------|-----------|-------------------|
| IRQ0     | 32        | PIT Timer         |
| IRQ1     | 33        | Keyboard          |
| IRQ2–15  | 34–47     | Other (masked)    |

Only IRQ0 and IRQ1 are unmasked (`PIC1 data = 0xFC`).  All other hardware
interrupts are silenced.

When the PIT fires, the PIC translates IRQ0 into **interrupt vector 32** and
signals the CPU.

---

## 3. The IDT and Interrupt Gate Contract

The CPU looks up vector 32 in the **Interrupt Descriptor Table** (IDT).  Each
IDT entry specifies:

- The **handler address** (64-bit, split across three fields)
- The **code segment selector** (`0x08` — kernel code)
- The **gate type** (`0x8E` = present + interrupt gate)

An *interrupt gate* is critical: it causes the CPU to **automatically clear the
IF (Interrupt Flag)** in RFLAGS before jumping to the handler.  This means:

> **While our IRQ handler is running, no further interrupts can preempt it.**

This is what prevents nested timer interrupts from corrupting the context switch.

### What the CPU Does on Interrupt Entry (Before Our Code Runs)

When vector 32 fires, the CPU performs these steps in hardware — we cannot
observe or control them:

1. **Clear IF** in RFLAGS (no more interrupts)
2. **Push onto the current stack** (in this exact order):
   - `SS`      (stack segment)
   - `RSP`     (stack pointer of the interrupted code)
   - `RFLAGS`  (flags of the interrupted code, with IF=1)
   - `CS`      (code segment of the interrupted code)
   - `RIP`     (instruction pointer — where to resume)
3. **Load RIP** from the IDT entry → jump to our handler stub

These five values form the `InterruptStackFrame`:

```rust
#[repr(C)]
pub struct InterruptStackFrame {
    pub rip: u64,       // ← pushed last (lowest address)
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,        // ← pushed first (highest address)
}
```

At this point, control transfers to the assembly stub.  The CPU has saved the
*minimum* state needed to return (RIP, CS, RFLAGS, RSP, SS), but **all 15
general-purpose registers still hold the interrupted task's values**.

---

## 4. The IRQ Assembly Stub — Where the Magic Happens

This is the most critical code in the entire multitasking system.  It bridges
the hardware interrupt into Rust and back, and it is where the actual stack
pointer swap happens.

```asm
irq0_pit_timer_stub:
    ; ── Phase 1: Save the interrupted task's registers ──
    cli                    ; (1) Redundant safety — CPU already cleared IF
    push rax               ; (2) Save all 15 general-purpose registers
    push rcx               ;     in a fixed order that matches the
    push rdx               ;     SavedRegisters struct layout.
    push rbx               ;
    push rbp               ;     After these 15 pushes, RSP points to
    push rsi               ;     the start of a SavedRegisters block
    push rdi               ;     that sits directly below the
    push r8                ;     InterruptStackFrame the CPU pushed.
    push r9                ;
    push r10               ;
    push r11               ;
    push r12               ;
    push r13               ;
    push r14               ;
    push r15               ;

    ; ── Phase 2: Call into Rust ──
    mov edi, 32            ; (3) arg0: vector number (IRQ0 = 32)
    mov rsi, rsp           ; (4) arg1: pointer to SavedRegisters on stack
    and rsp, -16           ; (5) 16-byte align RSP (SysV AMD64 ABI requirement)
    call irq_rust_dispatch ; (6) → Rust scheduler → returns *mut SavedRegisters

    ; ── Phase 3: Restore a (potentially different) task's registers ──
    mov rsp, rax           ; (7) KEY STEP: RSP = return value from Rust.
                           ;     If the scheduler chose a different task,
                           ;     RSP now points into THAT task's stack.
    pop r15                ; (8) Restore registers from the new stack
    pop r14                ;
    pop r13                ;
    pop r12                ;
    pop r11                ;
    pop r10                ;
    pop r9                 ;
    pop r8                 ;
    pop rdi                ;
    pop rsi                ;
    pop rbp                ;
    pop rbx                ;
    pop rdx                ;
    pop rcx                ;
    pop rax                ;

    ; ── Phase 4: Resume the task ──
    iretq                  ; (9) Pops RIP, CS, RFLAGS, RSP, SS
                           ;     → CPU jumps to the restored RIP
                           ;     → Interrupts re-enabled (RFLAGS has IF=1)
```

### The Critical Insight: Step (7)

Steps (2)–(6) save the current task's complete CPU state onto its stack and
call into Rust.  The Rust function `irq_rust_dispatch` returns a pointer to a
`SavedRegisters` block — but this pointer may point to a **different task's
stack**.

Step (7) — `mov rsp, rax` — is where the context switch physically happens.
From this instruction onward, the CPU is operating on a different stack.  The
subsequent `pop` instructions and `iretq` restore registers from that other
stack and resume the other task.

**The interrupted task does not know it was suspended.**  Its registers, stack,
and instruction pointer are frozen in the `SavedRegisters` + `InterruptStackFrame`
on its own stack.  When a future timer tick selects it again, the same mechanism
restores its state and it continues exactly where it left off.

---

## 5. The Rust Dispatch Layer

The assembly stub calls `irq_rust_dispatch`, which validates the vector and
delegates to the registered handler:

```
irq_rust_dispatch(vector=32, frame=RSP)
  └─► dispatch_irq(vector=32, frame)
        ├─► handler(32, frame)             // registered handler
        │     └─► timer_irq_handler()
        │           └─► on_timer_tick()    // scheduler core
        │                 returns: *mut SavedRegisters (next task)
        └─► end_of_interrupt(irq=0)        // acknowledge PIT to PIC
              returns: *mut SavedRegisters → RAX → assembly stub
```

The End-of-Interrupt (EOI) signal to the PIC is sent **after** the scheduler
runs but **before** `iretq`.  This tells the PIC that IRQ0 has been handled and
it may deliver the next one.

---

## 6. Scheduler Core: `on_timer_tick`

This function executes on every timer tick with interrupts disabled.  It
receives the current frame pointer (the `SavedRegisters` on the interrupted
task's stack) and returns the frame pointer of the task to resume.

```
on_timer_tick(current_frame) → next_frame
```

### Step-by-Step Logic

1. **Early exit**: If the scheduler is not started or has no tasks, return
   `current_frame` unchanged (the interrupted code resumes immediately).

2. **Identify the interrupted task**: `find_entry_by_frame()` scans all task
   slots to find which task's stack contains `current_frame`.  This works
   because each task has a known, disjoint 64 KiB stack region.

3. **Capture the bootstrap frame**: The very first timer tick interrupts the
   kernel REPL (which is not a scheduled task).  Its frame is saved as the
   `bootstrap_frame` — the context to restore when the scheduler stops.

4. **Handle stop request**: If `request_stop()` was called by a task, reset
   all scheduler state and return the `bootstrap_frame` to resume the REPL.

5. **Save the current task's frame**: Store `current_frame` into the task's
   `frame_ptr` slot so it can be restored on a future tick.

6. **Round-robin selection**: Starting from the position after the current
   task, iterate through the run queue and pick the next task with a valid
   frame pointer.

7. **Return the selected task's `frame_ptr`**: This pointer goes back through
   the dispatch chain into RAX, and the assembly stub uses it as the new RSP.

---

## 7. Returning a Different Frame Pointer — The Actual Context Switch

Here is what makes a context switch different from a normal interrupt return:

### Normal Interrupt (No Context Switch)
```
Timer fires → save regs → scheduler returns SAME frame → restore regs → iretq
Result: interrupted code resumes as if nothing happened.
```

### Context Switch
```
Timer fires while Task A runs
  → save Task A's regs onto Task A's stack
  → scheduler returns Task B's frame (pointing into Task B's stack)
  → restore Task B's regs from Task B's stack
  → iretq pops Task B's RIP/RFLAGS/RSP
  → Task B resumes where it was previously interrupted

Task A's state remains frozen on Task A's stack until a future tick selects it.
```

The key contract is that every task's stack, at the point where it was last
preempted, contains a complete `SavedRegisters` + `InterruptStackFrame` block.
The scheduler's `frame_ptr` for each task always points to the `SavedRegisters`
at the top of that block.

---

## 8. Synthetic Task Frames: Making `iretq` Boot a New Task

A newly spawned task has never been interrupted — it has no "previous context"
to restore.  The solution is to **construct a fake interrupt frame** on the
task's stack that looks exactly like what the assembly stub expects.

`build_initial_task_frame()` places two structures at the top of the task's
64 KiB stack:

```
┌────────────────────────────────────┐  ← stack_top (aligned to 16 bytes, −8)
│     InterruptStackFrame (40 B)     │
│  ┌──────────────────────────────┐  │
│  │ rip = task entry function    │  │  ← iretq will jump here
│  │ cs  = 0x08 (kernel code)     │  │
│  │ rflags = 0x202 (IF=1)        │  │  ← interrupts re-enabled after iretq
│  │ rsp = entry_rsp              │  │  ← stack pointer for the task
│  │ ss  = 0x10 (kernel data)     │  │
│  └──────────────────────────────┘  │
├────────────────────────────────────┤
│     SavedRegisters (120 B)         │
│  ┌──────────────────────────────┐  │
│  │ r15 = 0                      │  │
│  │ r14 = 0                      │  │
│  │ ...                          │  │  ← all zeros (clean initial state)
│  │ rax = 0                      │  │
│  └──────────────────────────────┘  │
├────────────────────────────────────┤  ← frame_ptr (stored in task slot)
│                                    │
│     ~65,376 bytes free stack       │  ← task execution grows downward
│                                    │
└────────────────────────────────────┘  ← stack_base
```

When the scheduler first selects this task, it returns `frame_ptr`.  The
assembly stub does:

1. `mov rsp, rax` — RSP now points to the synthetic `SavedRegisters`
2. `pop r15` ... `pop rax` — loads zeros into all GP registers
3. `iretq` — pops the synthetic `InterruptStackFrame`:
   - Loads RIP with the task entry function address
   - Loads RFLAGS with `0x202` (IF=1 — re-enables interrupts)
   - Loads RSP with the task's own stack pointer
   - **The task begins executing its first instruction.**

From the CPU's perspective, this is indistinguishable from returning to a
previously interrupted task.  The `iretq` instruction does not know or care
whether the frame was created by a real interrupt or by software.

### Why RFLAGS = 0x202?

- **Bit 1** (always 1): Reserved, must be set in RFLAGS.
- **Bit 9** (IF = 1): The Interrupt Flag.  When `iretq` loads this value into
  RFLAGS, it re-enables maskable interrupts.  This is essential — without IF=1
  the task would run with interrupts disabled and the next timer tick would
  never fire, making preemption impossible.

### Stack Page Pre-Touching

Before building the frame, `build_initial_task_frame` writes a zero byte to
every page of the task's stack:

```rust
for page_off in (0..TASK_STACK_SIZE).step_by(PAGE_SIZE) {
    ptr::write_volatile(stack.as_mut_ptr().add(page_off), 0);
}
```

This triggers demand paging (page fault → VMM allocates physical frame) during
`spawn()`, which runs with interrupts enabled in normal kernel context.  Without
pre-touching, the first access to an unmapped stack page would cause a page
fault **inside the timer IRQ handler** — a fatal double fault.

---

## 9. Stack Memory Layout Per Task

Each of the 8 task slots owns a 64 KiB stack region within the static
`SchedulerData.stacks` array.  The stacks live in the kernel's BSS segment
(zero-initialized at boot).

```
SchedulerData (static, ~513 KiB total)
├── meta: SchedulerMetadata (~200 B)
└── stacks: [8 × 64 KiB]
    ├── stacks[0]: Task 0 stack (64 KiB)
    ├── stacks[1]: Task 1 stack (64 KiB)
    ├── ...
    └── stacks[7]: Task 7 stack (64 KiB)
```

During task execution, the stack is used normally (function calls, local
variables).  When preempted, the CPU and the assembly stub push the interrupt
frame and saved registers onto this same stack, and the frame pointer is stored
in `TaskEntry.frame_ptr`.

---

## 10. Synchronization: Why Interrupts Must Be Disabled

The scheduler's shared state (`SchedulerData`) is accessed from two contexts:

1. **Normal kernel context**: `spawn()`, `start()`, `request_stop()`, `is_running()`
2. **Interrupt context**: `on_timer_tick()` called from the timer IRQ handler

On a single-core system, the only source of concurrency is interrupt preemption.
If a timer IRQ fires while `spawn()` is modifying the task list, the
`on_timer_tick()` handler would see partially updated state — a classic race
condition.

The `SpinLock` protects against this.  Its `lock()` method:

```rust
pub fn lock(&self) -> SpinLockGuard<'_, T> {
    let interrupts_were_enabled = interrupts::are_enabled();
    interrupts::disable();          // CLI — no timer IRQ can fire now
    // ... acquire atomic lock ...
    SpinLockGuard { lock: self, interrupts_were_enabled }
}
```

And `SpinLockGuard::drop()`:

```rust
fn drop(&mut self) {
    self.lock.locked.store(false, Ordering::Release);
    if self.interrupts_were_enabled {
        interrupts::enable();       // STI — timer IRQs can fire again
    }
}
```

Key properties:

- **Interrupts are disabled before the lock is acquired**, preventing the timer
  IRQ handler from attempting to acquire the same lock (which would deadlock on
  a single core — the spinning would never end because the holder cannot run).
- **The previous interrupt state is saved and restored**, so nested critical
  sections (lock acquired while interrupts are already disabled) work correctly.
- **The lock is released before interrupts are restored**, preventing a window
  where an IRQ could fire and find the lock still held.

The `with_sched()` helper wraps every scheduler operation in this lock:

```rust
fn with_sched<R>(f: impl FnOnce(&mut SchedulerData) -> R) -> R {
    let mut sched = SCHED.lock();   // interrupts disabled
    f(&mut *sched)                  // exclusive access guaranteed
}                                   // drop: unlock + restore interrupts
```

---

## 11. The Bootstrap Frame: Getting Back to the Kernel REPL

The scheduler does not manage the kernel's main execution path (the command
REPL running in `start_round_robin_demo`'s `while is_running() { hlt }` loop).
When the first timer tick fires, it interrupts this REPL loop — not a scheduled
task.

The scheduler detects this: `find_entry_by_frame()` returns `None` because the
frame pointer does not fall within any task's stack.  It then saves this frame as
the **bootstrap frame**:

```rust
if meta.bootstrap_frame.is_null() && detected_slot.is_none() {
    meta.bootstrap_frame = current_frame;
}
```

When a task calls `request_stop()` and the next timer tick processes the stop:

```rust
if meta.stop_requested {
    let return_frame = meta.bootstrap_frame;
    // ... reset all scheduler state ...
    return return_frame;
}
```

The assembly stub receives the bootstrap frame, restores the REPL's registers,
and `iretq` jumps back to the `hlt` instruction in the REPL loop.  The
`is_running()` check then returns `false` and the demo exits cleanly.

---

## 12. Complete Walkthrough: First Three Timer Ticks

Setup: Three tasks (A, B, C) have been spawned.  `current_queue_pos` is
initialized to `task_count - 1 = 2` (pointing at C) so that the first tick
advances to position 0 (Task A).

### Tick 1 — REPL → Task A

```
State: REPL loop is executing "hlt" with interrupts enabled.

1. PIT fires → PIC delivers vector 32 → CPU pushes InterruptStackFrame
   onto REPL's stack (the main kernel stack).
2. Assembly stub pushes 15 GP registers → RSP points to SavedRegisters
   on the REPL's stack.
3. irq_rust_dispatch(32, RSP) → on_timer_tick(RSP)
4. find_entry_by_frame: frame not in any task stack → detected_slot = None
5. bootstrap_frame is null → save current_frame as bootstrap_frame
6. Round-robin: (2 + 1) % 3 = position 0 → Task A selected
7. Return Task A's synthetic frame_ptr
8. Assembly: mov rsp, rax → RSP now points into Task A's stack
9. pop r15..rax → zeros loaded into all registers
10. iretq → RIP = demo_task_a, RFLAGS = 0x202 (IF=1)
11. Task A begins executing its first instruction.
```

### Tick 2 — Task A → Task B

```
State: Task A is running (writing 'A' to VGA, spinning in delay loop).

1. PIT fires → CPU pushes InterruptStackFrame onto Task A's stack.
2. Assembly stub pushes 15 registers → RSP = SavedRegisters on Task A's stack.
3. on_timer_tick(RSP)
4. find_entry_by_frame: frame is within stacks[0] → detected_slot = Some(0)
5. Save: slots[0].frame_ptr = current_frame
6. Round-robin: (0 + 1) % 3 = position 1 → Task B selected
7. Return Task B's frame_ptr (still the synthetic frame from spawn)
8. Assembly: mov rsp, rax → RSP now points into Task B's stack
9. pop + iretq → Task B starts executing.

Task A's complete state is frozen on its stack at slots[0].frame_ptr.
```

### Tick 3 — Task B → Task C

```
State: Task B is running.

1. PIT fires → save onto Task B's stack.
2. on_timer_tick: detected_slot = Some(1) → save Task B's frame
3. Round-robin: (1 + 1) % 3 = position 2 → Task C selected
4. Return Task C's frame_ptr → Task C starts executing.

After this, tick 4 wraps around: (2 + 1) % 3 = 0 → Task A is selected.
Task A resumes exactly where it was preempted in tick 2.
```

---

## 13. Source File Map

| File | Role |
|------|------|
| `src/arch/interrupts.rs` | IDT setup, PIC init, PIT programming, `irq_rust_dispatch`, `SavedRegisters` and `InterruptStackFrame` struct definitions |
| `src/arch/interrupts_stubs.rs` | Assembly macro stubs for all IRQ and exception vectors |
| `src/scheduler/roundrobin.rs` | Scheduler core: `init`, `spawn`, `start`, `on_timer_tick`, task frame construction |
| `src/scheduler/demotasks.rs` | Demo entry point and three example tasks |
| `src/sync/spinlock.rs` | `SpinLock<T>` with interrupt masking and RAII guard |
