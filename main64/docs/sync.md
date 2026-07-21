# KAOS Rust Synchronization Primitives - Technical Deep-Dive

This document provides a highly detailed, technical explanation of the synchronization primitives and scheduler-aware wait-queue mechanisms implemented in `src/sync`.

These primitives are designed for a `#![no_std]`, security-sensitive, x86_64 multi-core kernel environment operating in Ring 0.

---

## 1) Overview of Synchronization in the Kernel

Kernel code runs with high privileges and interacts directly with hardware interrupts. To prevent race conditions, deadlocks, and data corruption, KAOS implements a tiered synchronization architecture:

```text
┌────────────────────────────────────────────────────────────┐
│                    Raw Hardware / Atomics                  │
│                                                            │
│                  Atomic Variables & CAS                    │
│                        │          │                        │
└────────────────────────┼──────────┼────────────────────────┘
                         ▼          ▼
┌────────────────────────┐  ┌────────────────────────────────┐
│   SpinLock             │  │   RingBuffer (SPMC Lock-Free)  │
└──────────┬─────────────┘  └────────────────────────────────┘
           │
           ▼
┌────────────────────────────────────────────────────────────┐
│                  Scheduler-Aware Blocking                  │
│                                                            │
│   WaitQueue (Multi-Waiter)     SingleWaitQueue             │
│            │                          │                    │
│            ▼                          ▼                    │
│         ┌────────────────────────────────┐                 │
│         │       Scheduler Adapters       │                 │
│         └────────────────┬───────────────┘                 │
└──────────────────────────┼─────────────────────────────────┘
                           ▼
                  Task Blocking / sleep_if_*
```

1. **SpinLock**: Protects shared mutable data. Disables local CPU interrupts while held to prevent interrupt-handler deadlocks.
2. **RingBuffer**: Lock-free Single-Producer Multiple-Consumer (SPMC) circular buffer for interrupt-safe byte communication.
3. **SingleWaitQueue**: Non-allocating, single-waiter queue using an atomic sentinel, designed for simple producer/consumer top-half/bottom-half paths.
4. **WaitQueue**: Dynamic, multi-waiter queue using a heap-allocated list protected by an internal `SpinLock`.
5. **WaitQueue Adapters**: Bridges the gap between wait-queues and the task scheduler, handling atomic state transitions between `Blocked` and `Ready` states.

---

## 2) SpinLock (`spinlock.rs`)

The `SpinLock<T>` is the fundamental mutual exclusion primitive. It provides cooperative exclusion across different CPUs and CPU-to-interrupt contexts.

### 2.1 Struct Layout

```rust
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}
```

* `locked`: An `AtomicBool` representing the lock state (`true` = locked, `false` = unlocked).
* `data`: Wrapped in `UnsafeCell<T>` to allow interior mutability once the lock is acquired.

### 2.2 Interrupt Masking & The Deadlock Problem

If a CPU thread acquires a lock and is then interrupted by an interrupt service routine (ISR) that attempts to acquire the *same* lock, the CPU will spin forever on the lock. The ISR cannot finish because it is waiting for the lock, and the thread cannot release the lock because it was preempted by the ISR.

To prevent this deadlock:
1. **Disable Interrupts**: The `lock()` method disables interrupts on the local CPU *before* spinning.
2. **Restore Interrupts**: The `SpinLockGuard` records whether interrupts were previously enabled and restores that state when dropped.

### 2.3 Locking Protocol Flow Chart

```text
 Thread Context                    Local CPU Interrupts        SpinLock (AtomicBool)
 ══════════════                    ════════════════════        ═════════════════════
       │                                     │                           │
       ├─► Save interrupt state (are_enabled)│                           │
       │                                     │                           │
       ├─► disable() interrupts ────────────►│                           │
       │                                     │                           │
       │  [Loop: Try Acquire Lock]           │                           │
       ├───┐                                 │                           │
       │   ├─► compare_exchange(false->true) ───────────────────────────►│
       │  ◄┼─ Lock is Free (Success, Acquire Ordering) ──────────────────┤
       │   │                                 │                           │
       │   ├─► Lock is Busy (Error, Relaxed) ───────────────────────────►│
       │  ◄┼─ spin_loop() / yield CPU thread ────────────────────────────┤
       └───┘                                 │                           │
       │                                     │                           │
       ▼ [Returns SpinLockGuard]             │                           │
```

### 2.4 Lock Elision and Memory Ordering
* **Acquire Ordering** on lock acquisition (`compare_exchange`) guarantees that all memory reads/writes inside the critical section cannot be reordered before the lock is acquired.
* **Release Ordering** on lock release (`store(false, Ordering::Release)` in `drop()`) guarantees that all memory operations inside the critical section are committed and visible to subsequent processors before the lock is officially released.

---

## 3) Lock-Free Ring Buffer (`ringbuffer.rs`)

The `RingBuffer<const N: usize>` implements a lock-free Single-Producer Multiple-Consumer (SPMC) circular buffer. It allows safe communication between an ISR (producer) and one or more kernel threads (consumers) without disabling interrupts or using spinlocks.

### 3.1 Head and Tail Counters

```rust
pub struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    head_producer: AtomicUsize,
    tail_consumer: AtomicUsize,
}
```

* `head_producer` is the **write counter** updated exclusively by the single producer.
* `tail_consumer` is the **read counter** updated atomically by consumers.
* **Empty State**: `tail_consumer == head_producer`
* **Full State**: `head_producer.wrapping_sub(tail_consumer) == N` (utilizes the full capacity of `N` slots).

### 3.2 Single-Producer `push` (Lock-Free)

Since there is only a single producer, `push` does not require a Compare-and-Swap (CAS) loop:
1. It loads `head_producer` using `Relaxed` ordering (since only this thread writes to it).
2. It loads `tail_consumer` using `Acquire` ordering to observe the latest consumed indices.
3. If not full, it writes the byte to `buf[head]` inside an `unsafe` block.
4. It publishes the new write pointer using `Release` ordering:
   ```rust
   self.head_producer.store(next, Ordering::Release);
   ```
   This ensures the byte write is visible to consumers before they observe the updated head pointer.

### 3.3 Multi-Consumer `pop` (CAS Loop)

Because multiple consumers can call `pop()` concurrently, a CAS loop is required to prevent duplicate delivery (where two consumers read and return the same byte):

```text
           [Start pop]
                │
                ▼
      ┌──────────────────┐
      │ Load tail & head │◄──────────────────────┐
      │   with Acquire   │                       │
      └─────────┬────────┘                       │
                │                                │
                ▼                                │
        Is tail == head? ──(Yes)──► Return None  │
                │                                │
             (No)                                │
                ▼                                │
    ┌──────────────────────┐                     │
    │ Speculatively read   │                     │
    │     buf[tail % N]    │                     │
    └──────────┬───────────┘                     │
               │                                 │
               ▼                                 │
    ┌──────────────────────┐                     │
    │ Calculate next count │                     │
    │       tail + 1       │                     │
    └──────────┬───────────┘                     │
               │                                 │
               ▼                                 │
         CAS tail_consumer:                      │
           tail -> next?                         │
            /         \                          │
      (Success)     (Failure)                    │
          /             \                        │
         ▼               └───────────────────────┘
  Return Some(value)
```

* The speculative read of `buf[tail % N]` is safe because the producer only writes to `buf[head % N]` and publishes it after a `Release` barrier. Thus, any slot between `tail` and `head` contains valid, initialized data.
* **ABA Problem Prevention**: Because the indices are free-running monotonic counters rather than being bounded by `N`, a slot wrapped around the buffer has a completely different counter value. This inherently prevents the ABA problem in the CAS loop.
* If the CAS fails, it means another consumer successfully claimed the slot, so we retry.

---

## 4) Scheduler-Agnostic Wait Queues

Wait queues keep track of tasks waiting for a condition to become true (e.g., waiting for serial port input or a timer tick). They do not perform the context switch themselves; instead, they track waiter registrations.

### 4.1 SingleWaitQueue (`singlewaitqueue.rs`)

Designed for scenarios where at most one task waits at a time (e.g., a dedicated driver thread waiting for device interrupts).

* **Representation**: A single `AtomicUsize` storing the `task_id` of the waiter, or the sentinel `usize::MAX` (`NO_WAITER`).
* **Registration**: Uses `compare_exchange(NO_WAITER, task_id, Ordering::AcqRel, Ordering::Acquire)`. Returns `false` if another waiter is already registered.
* **Idempotency**: If the slot already contains `task_id`, registration succeeds and returns `true`.
* **Clearing**: Clears the waiter slot back to `NO_WAITER` only if it matches `task_id`, avoiding race conditions from late-clearing threads.

### 4.2 WaitQueue (`waitqueue.rs`)

A multi-waiter queue that can store an arbitrary number of waiting tasks in a heap-allocated `Vec<usize>`.

```rust
pub struct WaitQueue {
    state: SpinLock<WaitQueueState>,
}

struct WaitQueueState {
    waiters: Vec<usize>,
    wake_scratch: Vec<usize>,
}
```

#### Out-Of-Memory (OOM) Protection
`WaitQueue` registers waiters while holding an internal `SpinLock`. Since holding this lock disables interrupts, a panic during allocation (e.g., if the vector reallocates and runs out of memory) would halt the system or cause a kernel deadlock.
To mitigate this, `register_waiter` uses `try_reserve(1)`:
```rust
if state.waiters.try_reserve(1).is_err() {
    return false; // OOM - handled gracefully by the caller
}
state.waiters.push(task_id);
```

#### The Double-Buffer Recycling Wake Strategy (`wake_all`)
To avoid holding the `WaitQueue` spinlock during scheduler operations (which would cause massive lock contention and keep interrupts disabled for too long), `wake_all` uses a double-buffering scheme:

1. **Step 1 (Under Lock)**: Clear `wake_scratch`, swap `waiters` with `wake_scratch`, and extract `wake_scratch` using `core::mem::take`.
   ```rust
   let mut drained_waiters = {
       let mut state = self.state.lock();
       state.wake_scratch.clear();
       let WaitQueueState { waiters, wake_scratch } = &mut *state;
       core::mem::swap(waiters, wake_scratch);
       core::mem::take(&mut state.wake_scratch)
   };
   ```
2. **Step 2 (Lock Released)**: Process the wakeups outside the lock. For each drained `task_id`, execute the `wake` callback (which unblocks the task in the scheduler).
3. **Step 3 (Recycling)**: Re-acquire the lock and put `drained_waiters` back into `wake_scratch` if its capacity is larger. This recycles the allocated vector capacity and avoids future memory allocations.

---

## 5) Scheduler Adapters (`waitqueue_adapter.rs`)

The adapter module couples wait queues with the task scheduler's state machine.

### 5.1 Sleep Outcome State

```rust
pub enum SleepOutcome {
    Blocked,
    QueueFull,
    ConditionFalse,
}
```

* `Blocked`: The condition was true, the task was registered, and its scheduler state transitioned to `Blocked`.
* `QueueFull`: The condition was true, but registration failed (e.g., OOM in `WaitQueue` or slot occupied in `SingleWaitQueue`). The scheduler did not block the task, but the caller should yield the CPU to prevent lockups.
* `ConditionFalse`: The condition evaluated to `false` (i.e., the data is ready/event occurred). The task remains `Ready`.

### 5.2 Atomic Wait Protocol (`sleep_if_*`)

To block a task safely, we must prevent the **Lost Wakeup Problem**. This occurs if the condition changes (or an interrupt fires) after we check the condition but before we register as blocked.

The `sleep_if_multi` and `sleep_if_single` functions prevent this by performing the check and registration under local interrupt disablement:

```text
 Task Context                     Local CPU Interrupts    WaitQueue / SingleWaitQueue      Scheduler
 ════════════                     ════════════════════    ═══════════════════════════      ═════════
      │                                     │                          │                       │
      ├─► Save & disable interrupts ───────►│                          │                       │
      │                                     │                          │                       │
      │ [Evaluate should_block()]           │                          │                       │
      ├─► If Condition is TRUE:             │                          │                       │
      │   │                                 │                          │                       │
      │   ├─► register_waiter(task_id) ───────────────────────────────►│                       │
      │   │   ├──► Registration Succeeded (returns true) ──────────────┤                       │
      │   │   │    ├─► block_task(task_id) ───────────────────────────────────────────────────►│
      │   │   │    └─► Outcome = Blocked                               │                       │
      │   │   │                                                        │                       │
      │   │   └──► Registration Failed (OOM / Occupied, returns false)─┤                       │
      │   │        └─► Outcome = QueueFull                             │                       │
      │   │                                                            │                       │
      │   ▼                                 │                          │                       │
      ├─► If Condition is FALSE:            │                          │                       │
      │   │                                 │                          │                       │
      │   ├─► clear_waiter(task_id) ──────────────────────────────────►│                       │
      │   └─► Outcome = ConditionFalse      │                          │                       │
      │                                     │                          │                       │
      ├─► Restore interrupt state ─────────►│                          │                       │
      │                                     │                          │                       │
      ▼ [Yield CPU if Blocked or QueueFull] │                          │                       │
```

1. **Disable Interrupts**: Local interrupts are disabled.
2. **Evaluate Condition**: `should_block()` is evaluated.
3. **Register Waiter**: If `should_block` is true, the task is registered in the queue.
4. **Transition Scheduler State**: If registration succeeds, `scheduler::block_task(task_id)` changes the task state to `Blocked`.
5. **Restore Interrupts**: Local interrupts are restored.
6. **Yield**: If the task was blocked or the queue was full, the caller calls `yield_now()` to yield control.
