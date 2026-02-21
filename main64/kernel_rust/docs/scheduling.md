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
11. [The Bootstrap Frame and the Idle Loop](#11-the-bootstrap-frame-and-the-idle-loop)
12. [Complete Walkthrough: First Three Timer Ticks](#12-complete-walkthrough-first-three-timer-ticks)
13. [Task States and Blocking](#13-task-states-and-blocking)
14. [RingBuffer: Lock-Free SPMC Byte Queue](#14-ringbuffer-lock-free-spmc-byte-queue)
15. [WaitQueue and SingleWaitQueue](#15-waitqueue-and-singlewaitqueue)
16. [The waitqueue_adapter: Decoupling Queues from Scheduler](#16-the-waitqueue_adapter-decoupling-queues-from-scheduler)
17. [Lost-Wakeup Protection](#17-lost-wakeup-protection)
18. [Keyboard Architecture: Top-Half and Bottom-Half](#18-keyboard-architecture-top-half-and-bottom-half)
19. [Complete Walkthrough: A Keystroke Reaches a Task](#19-complete-walkthrough-a-keystroke-reaches-a-task)
20. [`yield_now`: Cooperative Rescheduling](#20-yield_now-cooperative-rescheduling)
21. [The Boot Sequence](#21-the-boot-sequence)
22. [Source File Map](#22-source-file-map)

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
   idle loop (which is not a scheduled task).  Its frame is saved as the
   `bootstrap_frame` — the context to restore when the scheduler stops.

4. **Handle stop request**: If `request_stop()` was called by a task, reset
   all scheduler state and return the `bootstrap_frame` to resume the idle loop.

5. **Save the current task's frame**: Store `current_frame` into the task's
   `frame_ptr` slot so it can be restored on a future tick.

6. **Round-robin selection**: Starting from the position after the current
   task, iterate through the run queue and pick the next task with a valid
   frame pointer.  **Blocked tasks are skipped** (see section 13).

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

`build_initial_kernel_task_frame()` places two structures at the top of the task's
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

Before building the frame, `build_initial_kernel_task_frame` writes a zero byte to
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

## 11. The Bootstrap Frame and the Idle Loop

After boot, `KernelMain` spawns all system tasks, starts the scheduler, enables
interrupts, and falls into a **low-power idle loop**:

```rust
fn idle_loop() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}
```

The idle loop is **not** a scheduled task — it runs on the main kernel stack.
When the first timer tick fires, it interrupts this `hlt` instruction.  The
scheduler detects that the interrupted frame does not belong to any task stack
and saves it as the **bootstrap frame**:

```rust
if meta.bootstrap_frame.is_null() && detected_slot.is_none() {
    meta.bootstrap_frame = current_frame;
}
```

The bootstrap frame serves two purposes:

1. **Fallback when all tasks are blocked**: When the scheduler finds no runnable
   task, the timer tick returns the bootstrap frame, resuming the `hlt` loop.
   The CPU halts in low-power mode until the next interrupt wakes a task.

2. **Restore point for `request_stop()`**: When a task calls `request_stop()`,
   the next timer tick resets all scheduler state and returns the bootstrap
   frame, resuming the idle loop.

Note: The REPL (command prompt) runs as a **scheduled task** (`repl_task`), not
as part of the idle loop.  The idle loop is purely a low-power wait that the CPU
enters when no task needs CPU time.

---

## 12. Complete Walkthrough: First Three Timer Ticks

Setup: Two tasks have been spawned — the keyboard worker (slot 0) and the REPL
(slot 1).  `current_queue_pos` is initialized to `task_count - 1 = 1` (pointing
at the REPL) so that the first tick advances to position 0 (keyboard worker).

### Tick 1 — Idle Loop → Keyboard Worker

```
State: idle_loop() is executing "hlt" with interrupts enabled.

1. PIT fires → PIC delivers vector 32 → CPU pushes InterruptStackFrame
   onto the main kernel stack.
2. Assembly stub pushes 15 GP registers → RSP points to SavedRegisters
   on the main kernel stack.
3. irq_rust_dispatch(32, RSP) → on_timer_tick(RSP)
4. find_entry_by_frame: frame not in any task stack → detected_slot = None
5. bootstrap_frame is null → save current_frame as bootstrap_frame
6. Round-robin: (1 + 1) % 2 = position 0 → keyboard worker selected
7. Return keyboard worker's synthetic frame_ptr
8. Assembly: mov rsp, rax → RSP now points into keyboard worker's stack
9. pop r15..rax → zeros loaded into all registers
10. iretq → RIP = keyboard_worker_task, RFLAGS = 0x202 (IF=1)
11. Keyboard worker begins executing its first instruction.
```

### Tick 2 — Keyboard Worker → REPL

```
State: Keyboard worker runs, finds no raw scancodes, blocks itself on
       RAW_WAITQUEUE, then calls yield_now().  yield_now() triggers
       "int 32" which enters the same on_timer_tick path.

1. Software "int 32" → CPU pushes InterruptStackFrame onto keyboard
   worker's stack.
2. Assembly stub pushes 15 registers.
3. on_timer_tick(RSP)
4. find_entry_by_frame: frame is within stacks[0] → detected_slot = Some(0)
5. Save: slots[0].frame_ptr = current_frame
6. Round-robin: start at (0 + 1) % 2 = position 1
   → Slot 0 (keyboard worker) is Blocked → skip
   → Slot 1 (REPL) is Ready → selected
7. Return REPL's frame_ptr (still the synthetic frame from spawn)
8. Assembly: mov rsp, rax → RSP now points into REPL's stack
9. pop + iretq → REPL starts executing: clears screen, prints banner.

Keyboard worker's complete state is frozen on its stack at slots[0].frame_ptr.
```

### Tick 3 — REPL Waiting for Input → Idle Loop

```
State: REPL calls read_char_blocking() → buffer empty → blocks itself on
       INPUT_WAITQUEUE → calls yield_now().

1. Software "int 32" → save REPL's frame onto REPL's stack.
2. on_timer_tick: detected_slot = Some(1)
3. Round-robin: both tasks are Blocked → no runnable task found
4. No task selected → return bootstrap_frame
5. Assembly: mov rsp, rax → RSP back to the main kernel stack
6. iretq → resumes in idle_loop() → executes hlt

The CPU halts in low-power mode.  Both tasks are blocked:
  - Keyboard worker waits for raw scancodes (RAW_WAITQUEUE)
  - REPL waits for decoded characters (INPUT_WAITQUEUE)

The next hardware event (keyboard IRQ) will wake the keyboard worker,
which decodes the scancode and wakes the REPL.
```

---

## 13. Task States and Blocking

Each task has a `TaskState` that controls whether the scheduler will consider
it during round-robin selection:

```rust
pub enum TaskState {
    Ready,    // eligible for scheduling
    Running,  // currently on the CPU
    Blocked,  // waiting for an external event — scheduler skips this task
}
```

### State Transitions

```
                  spawn()
                    │
                    ▼
               ┌─────────┐   on_timer_tick selects   ┌─────────┐
               │  Ready   │ ─────────────────────────►│ Running │
               └─────────┘                            └─────────┘
                    ▲                                      │
                    │         on_timer_tick preempts        │
                    └──────────────────────────────────────┘
                    ▲                                      │
    unblock_task()  │                                      │  block_task()
                    │         ┌─────────┐                  │
                    └─────────│ Blocked │◄─────────────────┘
                              └─────────┘
```

### How Blocking Works

When a task needs to wait for an event (e.g. keyboard input), it does not
busy-wait.  Instead, it marks itself as **Blocked** via `block_task()`:

```rust
pub fn block_task(task_id: usize) {
    with_sched(|sched| {
        sched.meta.slots[task_id].state = TaskState::Blocked;
    });
}
```

The scheduler's round-robin loop then **skips** blocked tasks:

```rust
for step in 0..meta.task_count {
    let pos = (search_start_pos + step) % meta.task_count;
    let slot = meta.run_queue[pos];

    // Skip blocked tasks — they are waiting for an external event.
    if meta.slots[slot].state == TaskState::Blocked {
        continue;
    }
    // ... select this task ...
}
```

When the event occurs (e.g. a key is pressed), the interrupt handler or another
task calls `unblock_task()` to set the state back to `Ready`.  The next timer
tick will then consider this task again.

If **all** tasks are blocked, no task is selected and the scheduler returns
the bootstrap frame, resuming the idle loop's `hlt` instruction.  The CPU stays
in low-power halt until the next interrupt wakes a task.

---

## 14. RingBuffer: Lock-Free SPMC Byte Queue

The kernel uses a lock-free ring buffer (`sync/ringbuffer.rs`) for
interrupt-safe producer/consumer communication.  The ring buffer is a
fixed-size circular array that supports **single producer, multiple consumer
(SPMC)** access without any locks.

### Data Structure

```rust
pub struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    head_producer: AtomicUsize,   // write index (producer side)
    tail_consumer: AtomicUsize,   // read index (consumer side)
}
```

### Memory Layout

Both indices advance with modular arithmetic (`% N`), wrapping from the last
slot back to index 0.  This makes the fixed-size array reusable cyclically:

```
   tail_consumer      head_producer
        ▼                   ▼
  ┌───┬───┬───┬───┬───┬───┬───┬───┐
  │   │ A │ B │ C │ D │   │   │   │   N = 8
  └───┴───┴───┴───┴───┴───┴───┴───┘
        ▲               ▲
      pop()           push()
   (consumers)      (producer)
```

After enough push/pop operations, both indices wrap around so that
`head_producer` is at a lower array index than `tail_consumer`:

```
         head_producer   tail_consumer
              ▼             ▼
  ┌───┬───┬───┬───┬───┬───┬───┬───┐
  │ X │ Y │   │   │   │   │ W │   │   N = 8
  └───┴───┴───┴───┴───┴───┴───┴───┘
  ← wrapped                  oldest
```

The buffer is **empty** when `tail_consumer == head_producer` and **full** when
`(head_producer + 1) % N == tail_consumer`.  One slot is intentionally wasted
to distinguish full from empty.

### push(): Single Producer

Only one writer may call `push()`.  No Compare-And-Swap (CAS) is needed
because exactly one producer advances `head_producer`:

```
push('E'):
  1. Read head_producer (= 5)
  2. Compute next = (5 + 1) % 8 = 6
  3. Read tail_consumer — if next == tail_consumer, buffer is full → return false
  4. Write 'E' into buf[5]                        ← before publishing!
  5. Store head_producer = 6 (Release ordering)    ← now consumers can see it
```

The `Release` ordering on step 5 guarantees that the write in step 4 is visible
to any consumer that subsequently loads `head_producer` with `Acquire`.

### pop(): Multiple Consumers with CAS

Multiple tasks may call `pop()` concurrently.  A **Compare-And-Swap (CAS)
loop** prevents two consumers from reading the same element:

```
Consumer 1: pop()                    Consumer 2: pop()
  1. Load tail_consumer (= 1)         1. Load tail_consumer (= 1)
  2. Load head_producer (= 5)         2. Load head_producer (= 5)
  3. Read buf[1] → 'A'                3. Read buf[1] → 'A'    (speculative!)
  4. CAS(tail, 1→2): SUCCESS ✓        4. CAS(tail, 1→2): FAIL ✗ (now tail=2)
  5. Return Some('A')                 5. Loop: reload tail=2, read buf[2]→'B'
                                      6. CAS(tail, 2→3): SUCCESS ✓
                                      7. Return Some('B')
```

Without CAS, both consumers would observe `tail_consumer = 1`, both read `'A'`,
and both advance `tail_consumer` to 2 — delivering `'A'` twice and losing `'B'`.

The CAS uses `compare_exchange_weak` which is allowed to spuriously fail.  On
failure, the consumer simply retries from the beginning of the loop.

### Why Speculative Read is Safe

In step 3, the consumer reads `buf[tail_consumer]` **before** confirming
ownership via CAS.  This is safe because:

- The producer only writes to `buf[head_producer]` *before* advancing
  `head_producer` with `Release` ordering.
- The consumer reads `head_producer` with `Acquire` ordering in step 2.
- Any slot between `tail_consumer` and `head_producer` has already been written
  by the producer and will not be overwritten until the consumer advances
  `tail_consumer` past it.

### Usage in the Keyboard Driver

Two ring buffers are used with different access patterns:

| Buffer | Capacity | Producer | Consumer(s) | Pattern |
|--------|----------|----------|-------------|---------|
| `KEYBOARD.raw` | 64 | IRQ handler (`handle_irq`) | Keyboard worker task | SPSC |
| `KEYBOARD.buffer` | 256 | Keyboard worker (`handle_scancode`) | Any task (`read_char_blocking`) | SPMC |

Both use the same `RingBuffer<N>` type.  For `raw`, the CAS in `pop()` is
technically unnecessary (only one consumer), but it adds no correctness issues.

---

## 15. WaitQueue and SingleWaitQueue

Wait queues are the mechanism through which tasks declare "I am waiting for
something" and other code declares "the something happened".  They are
**scheduler-agnostic** — they only track waiter registration using atomic
operations.  The actual blocking/unblocking of tasks is handled by the adapter
layer (section 16).

### WaitQueue\<N\> — Multiple Waiters, Wake-All Semantics

```rust
pub struct WaitQueue<const N: usize> {
    waiters: [AtomicBool; N],   // waiters[task_id] = true → task is waiting
}
```

An array of atomic booleans, indexed by task ID.  When task 3 registers as a
waiter, `waiters[3]` is set to `true`.  When `wake_all()` is called, it
iterates all N slots, atomically swaps each `true` to `false`, and calls the
wake callback for each one:

```
Initial state:  [false, false, false, true, false, false, true, false]
                                       ▲                   ▲
                                     task 3              task 6

wake_all(callback):
  slot 0: false → skip
  slot 1: false → skip
  slot 2: false → skip
  slot 3: swap(false) returns true → callback(3)
  slot 4: false → skip
  slot 5: false → skip
  slot 6: swap(false) returns true → callback(6)
  slot 7: false → skip

After:          [false, false, false, false, false, false, false, false]
```

The atomic `swap` guarantees that each waiter is woken exactly once, even if
`wake_all()` is called concurrently from multiple contexts.

**Usage**: `INPUT_WAITQUEUE` — multiple consumer tasks can wait for decoded
keyboard characters simultaneously.

### SingleWaitQueue — One Waiter, Atomic Slot

```rust
pub struct SingleWaitQueue {
    waiter: AtomicUsize,   // stores task_id or NO_WAITER (usize::MAX)
}
```

Optimized for the case where at most one task waits.  Uses a single
`AtomicUsize` instead of an array:

- **`register_waiter(task_id)`**: Atomically replaces `NO_WAITER` with
  `task_id` using CAS.  Fails if a different task is already registered.
  Succeeds silently if the same task re-registers (idempotent).

- **`wake_all(callback)`**: Atomically swaps `waiter` to `NO_WAITER`.
  If the previous value was a valid task ID, calls `callback(task_id)`.

```
register_waiter(2):
  CAS(NO_WAITER → 2): SUCCESS → waiter = 2

wake_all(callback):
  swap(NO_WAITER) → returns 2 → callback(2) → waiter = NO_WAITER
```

**Usage**: `RAW_WAITQUEUE` — only the keyboard worker task waits for raw
scancodes from the IRQ handler.  Using `SingleWaitQueue` instead of
`WaitQueue<8>` saves 7 bytes of atomic storage and avoids the O(N) iteration
in `wake_all`.

### Why Wake-All Instead of Wake-One?

When a keyboard character becomes available, **all** waiting tasks are woken
(thundering herd).  Each woken task attempts `read_char()` — a `pop()` from the
ring buffer with CAS.  Only one task succeeds; the others find the buffer empty
and go back to sleep.

This is simpler than wake-one because it avoids the need to track which task
should receive which character.  The CAS in `pop()` ensures correct delivery
regardless of how many tasks race for the same element.

---

## 16. The waitqueue_adapter: Decoupling Queues from Scheduler

The architecture deliberately separates three concerns:

```
┌─────────────────────────────────────────────────────────┐
│  Layer 3: Consumer Code (keyboard.rs, apps/mod.rs)      │
│  Uses: sleep_if_multi(), wake_all_multi(), yield_now()  │
├─────────────────────────────────────────────────────────┤
│  Layer 2: waitqueue_adapter.rs                          │
│  Couples: WaitQueue registration ↔ Scheduler states     │
│  Uses: queue.register_waiter() + scheduler::block_task()│
│  Uses: queue.wake_all(scheduler::unblock_task)          │
├──────────────────────┬──────────────────────────────────┤
│  Layer 1a:           │  Layer 1b:                       │
│  WaitQueue /         │  Scheduler                       │
│  SingleWaitQueue     │  (roundrobin.rs)                 │
│                      │                                  │
│  Knows: atomic flags │  Knows: TaskState, SpinLock      │
│  Does NOT know about │  Does NOT know about             │
│  TaskState or        │  WaitQueues or atomic flags      │
│  scheduler           │                                  │
└──────────────────────┴──────────────────────────────────┘
```

**Why this separation matters:**

1. **WaitQueues are testable in isolation** — they use only atomic operations,
   no scheduler dependency.  Unit tests can call `register_waiter()` and
   `wake_all()` without initializing the scheduler.

2. **The scheduler is testable in isolation** — `block_task()` and
   `unblock_task()` can be called directly in tests without any WaitQueue.

3. **The adapter is the single point of coupling** — only
   `waitqueue_adapter.rs` imports both `scheduler` and `waitqueue`.  If the
   scheduler's blocking API changes, only the adapter needs updating.

### The `sleep_if_*` Pattern

The adapter provides conditional blocking with lost-wakeup protection:

```rust
pub fn sleep_if_multi<const N: usize>(
    queue: &WaitQueue<N>,
    task_id: usize,
    should_block: impl FnOnce() -> bool,
) -> bool {
    let were_enabled = interrupts::are_enabled();
    interrupts::disable();              // ① no IRQs between check and block

    let mut blocked = should_block();   // ② check condition (e.g. buffer empty?)
    if blocked {
        queue.register_waiter(task_id); // ③ register in wait queue
        scheduler::block_task(task_id); // ④ mark task as Blocked
    } else {
        queue.clear_waiter(task_id);    // ⑤ clean up stale registration
    }

    if were_enabled {
        interrupts::enable();           // ⑥ restore interrupt state
    }
    blocked
}
```

The **else branch** (step ⑤) handles the case where a previous iteration
registered the waiter but the condition is now false (data arrived between
iterations).  Without clearing, a stale registration would remain.

### The `wake_all_*` Pattern

Waking is straightforward — it delegates to the queue's `wake_all` with
`scheduler::unblock_task` as the callback:

```rust
pub fn wake_all_multi<const N: usize>(queue: &WaitQueue<N>) {
    queue.wake_all(scheduler::unblock_task);
}
```

For each registered waiter, this atomically clears the waiter flag and calls
`unblock_task(task_id)`, which sets `TaskState::Ready` under the scheduler's
spinlock.

---

## 17. Lost-Wakeup Protection

The most subtle bug in any sleep/wake system is the **lost wakeup**:

```
Task (wants to sleep):              IRQ handler (wants to wake):

  1. Check: is buffer empty?  YES
                                      2. Push data into buffer
                                      3. wake_all() → no waiters registered!
  4. Register as waiter
  5. Block task
  6. yield_now()

  → DEADLOCK: Task is blocked, but nobody will ever wake it.
     The data sits in the buffer, unconsumed.
```

The problem: between checking the condition (step 1) and registering as a
waiter (step 4), an IRQ handler pushed data and called wake.  But the task was
not yet registered, so the wake had no effect.

### The Solution: Disable Interrupts Around Check + Register + Block

The `sleep_if_*` functions in the adapter disable interrupts **before** checking
the condition:

```
Task (wants to sleep):              IRQ handler (wants to wake):

  1. CLI (disable interrupts)
  2. Check: is buffer empty?  YES
     ┌─────────────────────────────────────────────────────────┐
     │ IRQ CANNOT fire here — interrupts are disabled!         │
     │ Step 3 from the IRQ handler cannot happen between       │
     │ our check and our registration.                         │
     └─────────────────────────────────────────────────────────┘
  3. Register as waiter
  4. Block task
  5. STI (restore interrupts)
  6. yield_now()
                                      7. IRQ fires now
                                      8. Push data into buffer
                                      9. wake_all() → finds waiter → unblocks

  → CORRECT: Task is woken and can consume the data.
```

Because interrupts are disabled during the entire check-register-block sequence,
no IRQ can fire in the critical window.  The wake happens **after** the task has
registered, so it takes effect.

---

## 18. Keyboard Architecture: Top-Half and Bottom-Half

The keyboard driver uses a **two-phase interrupt processing** model inspired
by Linux:

```
┌──────────────────────────────────────────────────────────────┐
│                    Hardware Layer                             │
│                                                              │
│  PS/2 Keyboard ──► 8042 Controller ──► PIC IRQ1 ──► Vector 33│
└──────────────────────────────────────┬───────────────────────┘
                                       │
                                       ▼
┌──────────────────────────────────────────────────────────────┐
│  TOP-HALF: handle_irq()             (IRQ context)            │
│                                                              │
│  Runs with interrupts DISABLED.  Must be fast.               │
│                                                              │
│  1. Read status register (port 0x64)                         │
│  2. Read scancode byte (port 0x60)                           │
│  3. Push raw scancode into KEYBOARD.raw ring buffer          │
│  4. Wake keyboard worker: wake_all_single(&RAW_WAITQUEUE)    │
│                                                              │
│  Time budget: ~microseconds.  No decoding, no state machine. │
└──────────────────────────────────────┬───────────────────────┘
                                       │ wakes
                                       ▼
┌──────────────────────────────────────────────────────────────┐
│  BOTTOM-HALF: keyboard_worker_task() (scheduled task)        │
│                                                              │
│  Runs as a normal preemptible task.  Can take as long as     │
│  needed without blocking other IRQs.                         │
│                                                              │
│  1. Drain all raw scancodes from KEYBOARD.raw                │
│  2. For each scancode:                                       │
│     - Track modifier state (Shift, CapsLock, Ctrl)           │
│     - Look up in scancode table (QWERTZ)                     │
│     - Push decoded ASCII into KEYBOARD.buffer                │
│  3. Wake consumer tasks: wake_all_multi(&INPUT_WAITQUEUE)    │
│  4. Sleep on RAW_WAITQUEUE until next IRQ wakes us           │
└──────────────────────────────────────┬───────────────────────┘
                                       │ wakes
                                       ▼
┌──────────────────────────────────────────────────────────────┐
│  CONSUMER: repl_task() / any task    (scheduled task)        │
│                                                              │
│  read_char_blocking():                                       │
│  1. Try pop() from KEYBOARD.buffer                           │
│  2. If empty → sleep on INPUT_WAITQUEUE → yield              │
│  3. On wakeup → retry pop()                                  │
│  4. Return decoded ASCII character                           │
└──────────────────────────────────────────────────────────────┘
```

### Why Two Phases?

**Top-half** (IRQ handler) runs with **interrupts disabled**.  While it
executes, no other IRQ can fire — not the timer (blocking preemption) and not
another keyboard interrupt (potentially losing keystrokes).  Therefore, the
top-half must complete in microseconds.

If the top-half did the full scancode decoding (shift state tracking, scancode
table lookup, caps lock toggling), it would hold interrupts disabled for much
longer.  With multiple modifiers and extended scancode sequences, this could
add significant latency to the timer and other IRQ handlers.

**Bottom-half** (keyboard worker task) runs as a normal scheduled task with
interrupts **enabled**.  It can be preempted by the timer, so other tasks keep
running.  The scancode decoding can take as long as needed without affecting
system responsiveness.

### Data Flow Through the Driver

```
                        IRQ context          Task context          Task context
                     ┌──────────────┐    ┌────────────────┐    ┌──────────────┐
Keyboard hardware    │  handle_irq  │    │ worker_task    │    │ consumer     │
    │                │              │    │                │    │ (REPL etc.)  │
    │ scancode       │   push(raw)  │    │  pop(raw)      │    │              │
    └───────────────►│──────┐       │    │──────┐         │    │              │
                     │      ▼       │    │      ▼         │    │              │
                     │  ┌───────┐   │    │  decode()      │    │              │
                     │  │KEYBOARD│   │    │      │         │    │              │
                     │  │ .raw  │───────►│      ▼         │    │              │
                     │  └───────┘   │    │  push(decoded) │    │  pop(decoded)│
                     │              │    │──────┐         │    │──────┐       │
                     │              │    │      ▼         │    │      ▼       │
                     │              │    │  ┌────────┐    │    │  return char │
                     │              │    │  │KEYBOARD│    │    │              │
                     │              │    │  │.buffer │────────►│              │
                     │              │    │  └────────┘    │    │              │
                     └──────────────┘    └────────────────┘    └──────────────┘

                     SingleWaitQueue      WaitQueue<8>
                     RAW_WAITQUEUE        INPUT_WAITQUEUE
                       (1 waiter)          (N waiters)
```

---

## 19. Complete Walkthrough: A Keystroke Reaches a Task

This walkthrough traces a single key press ('A') from hardware through all
layers to the REPL task.  Starting state: both the keyboard worker and the REPL
are **blocked** — the keyboard worker on `RAW_WAITQUEUE`, the REPL on
`INPUT_WAITQUEUE`.  The CPU is in the idle loop executing `hlt`.

### Phase 1: Hardware → Top-Half

```
1. User presses the 'A' key on the PS/2 keyboard.
2. The 8042 keyboard controller asserts IRQ1 on the PIC.
3. PIC translates IRQ1 to CPU interrupt vector 33.
4. CPU pushes InterruptStackFrame onto the idle loop's stack.
5. CPU clears IF (interrupts disabled) and jumps to the IRQ1 stub.
6. Assembly stub saves registers, calls irq_rust_dispatch(33, RSP).
```

### Phase 2: Top-Half Executes

```
7.  handle_irq() reads port 0x64 → status register says output buffer full.
8.  handle_irq() reads port 0x60 → raw scancode 0x1E (make code for 'A').
9.  KEYBOARD.raw.push(0x1E) → scancode stored in raw ring buffer.
10. wake_all_single(&RAW_WAITQUEUE):
      → swap(NO_WAITER) returns keyboard worker's task_id (0)
      → calls scheduler::unblock_task(0)
      → scheduler acquires SpinLock, sets slots[0].state = Ready
11. IRQ1 handler returns → EOI sent to PIC → iretq back to idle loop.
```

### Phase 3: Timer Tick Dispatches Keyboard Worker

```
12. ~1-4 ms later: PIT fires IRQ0 (vector 32).
13. on_timer_tick: idle loop frame → bootstrap (already saved).
14. Round-robin: slot 0 (keyboard worker) is Ready → selected.
    Slot 1 (REPL) is still Blocked → skipped.
15. Return keyboard worker's frame_ptr.
16. iretq → keyboard worker resumes where it called yield_now().
```

### Phase 4: Bottom-Half Decodes Scancode

```
17. Keyboard worker returns from yield_now(), loops back.
18. KEYBOARD.raw.pop() → returns Some(0x1E).
19. handle_scancode(0x1E):
      → 0x1E is a make code (bit 7 = 0) → handle_make(0x1E)
      → shift = false, caps_lock = false → use SCANCODES_LOWER
      → SCANCODES_LOWER[0x1E] = b'a'
      → KEYBOARD.buffer.push(b'a') → decoded character stored.
20. KEYBOARD.raw.pop() → returns None (no more scancodes).
21. decoded_any = true, buffer not empty →
    wake_all_multi(&INPUT_WAITQUEUE):
      → iterates waiters[0..8]
      → waiters[1] = true (REPL's task_id) → swap(false)
      → calls scheduler::unblock_task(1)
      → scheduler sets slots[1].state = Ready
22. sleep_if_single(&RAW_WAITQUEUE, 0, || raw.is_empty()):
      → interrupts disabled
      → raw.is_empty() = true
      → register_waiter(0) on RAW_WAITQUEUE → CAS(NO_WAITER → 0)
      → scheduler::block_task(0) → slots[0].state = Blocked
      → interrupts restored
      → returns true → keyboard worker calls yield_now()
```

### Phase 5: Timer Tick Dispatches REPL

```
23. yield_now() triggers "int 32" → on_timer_tick.
24. Round-robin: slot 0 (keyboard worker) is Blocked → skip.
    Slot 1 (REPL) is Ready → selected.
25. Return REPL's frame_ptr.
26. iretq → REPL resumes where it called yield_now().
```

### Phase 6: REPL Receives Character

```
27. REPL returns from yield_now(), loops back in read_char_blocking().
28. read_char() → KEYBOARD.buffer.pop() → CAS succeeds → returns Some(b'a').
29. read_char_blocking() returns b'a' to read_line().
30. read_line() echoes 'a' to the VGA screen.
31. read_line() loops → calls read_char_blocking() again → buffer empty →
    registers on INPUT_WAITQUEUE → blocks → yield_now().
32. System returns to idle state: both tasks blocked, CPU in hlt.
```

### Summary: Complete Path

```
Key press
  → IRQ1
    → handle_irq(): push raw scancode, wake worker         (~us, IRQ ctx)
      → Timer tick
        → on_timer_tick: select keyboard worker             (~us, IRQ ctx)
          → keyboard_worker_task: decode, push ASCII, wake REPL  (task ctx)
            → yield_now()
              → on_timer_tick: select REPL                  (~us, IRQ ctx)
                → read_char_blocking(): pop 'a', echo to screen  (task ctx)
```

Total latency from key press to character on screen: approximately 1-8 ms
(depending on when the next timer tick fires after the IRQ).

---

## 20. `yield_now`: Cooperative Rescheduling

In addition to preemptive scheduling (forced by the timer), tasks can
**voluntarily** give up the CPU.  This is critical for the sleep/wake pattern:
after a task marks itself as blocked, it must immediately trigger a reschedule
so the scheduler can select a different task.

```rust
pub fn yield_now() {
    unsafe {
        asm!(
            "int {vector}",
            vector = const interrupts::IRQ0_PIT_TIMER_VECTOR,
            options(nomem)
        );
    }
}
```

`yield_now()` triggers a **software interrupt** to the same vector as the PIT
timer (vector 32).  This enters the identical code path: assembly stub saves
registers, calls `on_timer_tick`, which selects the next task and returns a
different frame pointer.

```
Task A calls yield_now()
  │
  ▼
"int 32"
  │
  ▼ (same path as hardware timer)
CPU pushes InterruptStackFrame onto Task A's stack
Assembly stub pushes SavedRegisters
  │
  ▼
on_timer_tick(Task A's frame)
  → Task A is Blocked → skip
  → Task B is Ready → select
  → return Task B's frame_ptr
  │
  ▼
Assembly: mov rsp, rax (switch to Task B's stack)
pop registers, iretq → Task B resumes

When Task A is later unblocked and selected by a future timer tick:
  → iretq returns to the instruction after "int 32" in yield_now()
  → yield_now() returns to the caller
  → caller re-checks the condition (e.g. buffer still empty?) and either
    consumes data or sleeps again.
```

**Note**: Because `yield_now()` reuses the timer vector, each call also
increments the scheduler's `tick_count`.  This means `tick_count` reflects
total scheduling events (timer ticks + voluntary yields), not wall-clock time.

---

## 21. The Boot Sequence

The kernel boots through a strictly ordered initialization sequence.  Each step
depends on the previous ones being complete:

```
KernelMain(kernel_size)
  │
  ├─ serial::init()                          Debug output via COM1
  ├─ pmm::init()                             Physical Memory Manager
  ├─ interrupts::init()                      IDT, PIC, exception handlers
  ├─ vmm::init()                             Virtual Memory Manager (new CR3)
  ├─ heap::init()                            Kernel heap allocator
  │
  ├─ register_irq_handler(IRQ1, ...)         Keyboard IRQ → handle_irq()
  ├─ init_periodic_timer(250)                PIT fires at 250 Hz
  ├─ keyboard::init()                        Clear ring buffers
  │
  │  ┌─ scheduler::init()                    Reset scheduler, register timer handler
  │  ├─ scheduler::spawn(keyboard_worker)    → task slot 0
  │  ├─ scheduler::spawn(repl_task)          → task slot 1
  │  └─ scheduler::start()                   Mark scheduler as running
  │
  ├─ interrupts::enable()                    STI — first timer tick fires soon
  │
  └─ idle_loop()                             loop { hlt }
       │
       │  ← first timer tick interrupts here
       │
       ▼
     on_timer_tick: saves idle_loop frame as bootstrap,
                    selects keyboard worker → first task begins executing.
```

**Key ordering constraints:**

- `interrupts::init()` must come before `vmm::init()` because the VMM switches
  CR3, which could page-fault.  Exception handlers must already be installed.
- `init_periodic_timer(250)` is called **before** the scheduler is initialized,
  but interrupts are still disabled — no timer tick can fire yet.
- `scheduler::spawn()` builds synthetic frames on each task's stack.  The stack
  pages are pre-touched (written to) during spawn, triggering demand paging
  while interrupts are disabled.  This is safe because page faults are
  exceptions (not maskable interrupts).
- `interrupts::enable()` is the "point of no return" — from this moment,
  timer ticks fire and the scheduler takes over execution flow.

---

## 22. Source File Map

| File | Role |
|------|------|
| `src/arch/interrupts.rs` | IDT setup, PIC init, PIT programming, `irq_rust_dispatch`, `SavedRegisters` and `InterruptStackFrame` struct definitions |
| `src/arch/interrupts_stubs.rs` | Assembly macro stubs for all IRQ and exception vectors |
| `src/scheduler/roundrobin.rs` | Scheduler core: `init`, `spawn`, `start`, `on_timer_tick`, task frame construction, `block_task`, `unblock_task`, `current_task_id`, `yield_now` |
| `src/sync/spinlock.rs` | `SpinLock<T>` with interrupt masking and RAII guard |
| `src/sync/ringbuffer.rs` | Lock-free SPMC `RingBuffer<N>` with CAS-based `pop()` |
| `src/sync/waitqueue.rs` | `WaitQueue<N>` — multi-waiter, wake-all, scheduler-agnostic |
| `src/sync/singlewaitqueue.rs` | `SingleWaitQueue` — single-waiter, atomic slot, scheduler-agnostic |
| `src/sync/waitqueue_adapter.rs` | `sleep_if_multi/single`, `wake_all_multi/single` — couples wait queues to scheduler state transitions |
| `src/drivers/keyboard.rs` | PS/2 keyboard driver: top-half `handle_irq`, bottom-half `keyboard_worker_task`, consumer API `read_char_blocking` / `read_line` |
| `src/main.rs` | `KernelMain` boot sequence, `idle_loop`, `repl_task`, `command_prompt_loop` |

---

## 23. Task-Slot Storage Trade-Off

`SchedulerMetadata::slots` is intentionally implemented as `Vec<TaskEntry>`
with a per-entry `used` flag, not as a dedicated slot allocator.

Consequences:

- Spawn reuses free interior slots via first-fit search.
- Remove trims only trailing unused entries (`truncate` at last live slot).
- Interior holes are not compacted out of `slots`.
- Under heavy spawn/despawn churn, `slots.len()` can track a high-water mark
  even when many interior slots are free.

This is a deliberate simplicity trade-off:

- Pros: stable slot indices, straightforward `run_queue` handling, low mutation
  complexity in scheduler hot paths.
- Cons: metadata capacity can remain above live-task count until tail slots are
  released.
