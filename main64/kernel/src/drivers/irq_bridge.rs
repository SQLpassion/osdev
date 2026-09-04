//! Hardware IRQ bridge for user-space device drivers.
//!
//! Maps PIC IRQ lines (0..15) to Ring-3 driver tasks, executes minimal Ring-0
//! trampolines that wake waiting driver tasks on interrupt, and mediates PIC EOI
//! acknowledgment on user-space request (`IrqAck`).

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::arch::interrupts::pic::{end_of_interrupt, is_in_service};
use crate::arch::interrupts::types::{IRQ_BASE, IRQ_LINES};
use crate::arch::interrupts::{
    register_irq_handler, registered_irq_handler, IrqHandler, SavedRegisters,
};
use crate::scheduler;
use crate::sync::singlewaitqueue::SingleWaitQueue;
use crate::sync::waitqueue_adapter::{sleep_if_single, wake_all_single};
use crate::syscall::SyscallError;

/// Number of hardware PIC IRQ vectors supported (IRQ0..IRQ15).
pub const IRQ_COUNT: usize = IRQ_LINES;

/// Per-vector binding between a hardware IRQ line and the driver task waiting for it.
pub struct IrqBinding {
    /// Packed task ID of the subscribed driver task. 0 = unsubscribed.
    pub task_id: AtomicUsize,
    /// Set to true by the trampoline when an IRQ fires; cleared by IrqWait.
    pub pending: AtomicBool,
    /// The driver task blocks on this queue in IrqWait.
    pub waitq: SingleWaitQueue,
}

impl IrqBinding {
    /// Creates a new, unbound IRQ binding entry.
    pub const fn new() -> Self {
        Self {
            task_id: AtomicUsize::new(0),
            pending: AtomicBool::new(false),
            waitq: SingleWaitQueue::new(),
        }
    }
}

impl Default for IrqBinding {
    fn default() -> Self {
        Self::new()
    }
}

/// Static binding table — one entry per PIC IRQ line (0..15).
static IRQ_BINDINGS: [IrqBinding; IRQ_COUNT] = [
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
    IrqBinding::new(),
];

/// Maps a vector number (either raw IRQ line 0..15 or IDT vector 32..47) to IRQ line index 0..15.
pub fn irq_to_index(vector: u8) -> Option<usize> {
    if (IRQ_BASE..IRQ_BASE + (IRQ_COUNT as u8)).contains(&vector) {
        Some((vector - IRQ_BASE) as usize)
    } else if (vector as usize) < IRQ_COUNT {
        Some(vector as usize)
    } else {
        None
    }
}

/// Maps an IRQ line index (0..15) to its corresponding IDT interrupt vector (32..47).
pub fn irq_index_to_vector(index: usize) -> u8 {
    (IRQ_BASE as usize + index) as u8
}

/// Returns whether an IRQ line currently has an active user-space driver binding.
pub fn is_driver_irq(irq: u8) -> bool {
    if (irq as usize) < IRQ_COUNT {
        IRQ_BINDINGS[irq as usize].task_id.load(Ordering::Acquire) != 0
    } else {
        false
    }
}

/// Generic Ring-0 IRQ trampoline registered for driver-subscribed vectors.
///
/// Runs in top-half interrupt context — strictly lock-free and non-allocating.
/// Does NOT send PIC EOI: the device asserts the line until serviced by the driver,
/// so sending EOI early would produce spurious interrupts before the driver reads ISR.
pub fn driver_irq_trampoline(vector: u8, regs: &mut SavedRegisters) -> *mut SavedRegisters {
    // Step 1: Map incoming vector to direct IRQ line index.
    let idx = match irq_to_index(vector) {
        Some(i) => i,
        None => return regs as *mut SavedRegisters,
    };
    let binding = &IRQ_BINDINGS[idx];

    // Step 2: Mark IRQ pending for consumer task in Ring 3.
    binding.pending.store(true, Ordering::Release);

    // Step 3: Wake the driver task blocked on this IRQ's wait queue.
    wake_all_single(&binding.waitq);

    regs as *mut SavedRegisters
}

/// Subscribes `task_id` to receive notifications for `vector`.
pub fn subscribe(vector: u8, task_id: usize) -> Result<(), SyscallError> {
    let idx = irq_to_index(vector).ok_or(SyscallError::InvalidArg)?;
    let binding = &IRQ_BINDINGS[idx];

    // Step 1: Atomically claim exclusive ownership of the vector slot.
    if binding
        .task_id
        .compare_exchange(0, task_id, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(SyscallError::InvalidArg);
    }

    let idt_vector = irq_index_to_vector(idx);

    // Step 2: Refuse to silently steal a line already serviced by a
    // kernel-internal handler (e.g. `ata`'s IRQ14 handler, registered at
    // boot). On real hardware a legacy PCI interrupt line is routinely
    // shared between the device a driver task was granted and an unrelated
    // device the kernel already services directly; overwriting that handler
    // here would break the kernel's own device silently. Re-claiming a line
    // this bridge itself registered earlier (e.g. respawning the same
    // driver) is fine, since `driver_irq_trampoline` is idempotent to
    // re-register.
    if let Some(existing) = registered_irq_handler(idt_vector) {
        if !core::ptr::fn_addr_eq(existing, driver_irq_trampoline as IrqHandler) {
            binding.task_id.store(0, Ordering::Release);
            return Err(SyscallError::PermissionDenied);
        }
    }

    // Step 3: Register the top-half trampoline for this hardware vector.
    register_irq_handler(idt_vector, driver_irq_trampoline);

    Ok(())
}

/// Blocks `task_id` until an IRQ fires on `vector` or an event is already pending.
pub fn wait(vector: u8, task_id: usize, _timeout_ms: u32) -> Result<(), SyscallError> {
    let idx = irq_to_index(vector).ok_or(SyscallError::InvalidArg)?;
    let binding = &IRQ_BINDINGS[idx];

    // Step 1: Verify that the caller is the subscribed owner of this IRQ line.
    if binding.task_id.load(Ordering::Acquire) != task_id {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: If an IRQ has already fired and is pending, consume it immediately.
    if binding.pending.swap(false, Ordering::AcqRel) {
        return Ok(());
    }

    // Step 3: Sleep on the single wait queue until the top-half trampoline signals an IRQ.
    loop {
        let outcome = sleep_if_single(&binding.waitq, task_id, || {
            !binding.pending.load(Ordering::Acquire)
        });

        if outcome.should_yield() {
            scheduler::yield_now();
        }

        if binding.pending.swap(false, Ordering::AcqRel) {
            break;
        }
    }

    Ok(())
}

/// Acknowledges an IRQ event and sends the PIC EOI command.
pub fn ack(vector: u8, task_id: usize) -> Result<(), SyscallError> {
    let idx = irq_to_index(vector).ok_or(SyscallError::InvalidArg)?;
    let binding = &IRQ_BINDINGS[idx];

    // Step 1: Verify ownership of this IRQ vector before acknowledging.
    if binding.task_id.load(Ordering::Acquire) != task_id {
        return Err(SyscallError::InvalidArg);
    }

    // Step 2: Send PIC EOI now that the user driver has serviced device registers.
    end_of_interrupt(idx as u8);

    Ok(())
}

/// Releases every IRQ binding owned by `task_id`.
///
/// Called from the scheduler's `remove_task` — the single choke point reached
/// by both explicit termination (`Exit`/`terminate_task`) and zombie-reaping
/// after a crash (`#PF`/`#GP`/`#DE`) — so a driver task's death never leaves a
/// stale owner behind. Without this, `IRQ_BINDINGS[idx].task_id` would keep
/// pointing at the dead task forever: `subscribe()`'s
/// `compare_exchange(0, ...)` would fail for any future subscriber on that
/// vector, and `is_driver_irq()` would keep suppressing `dispatch_irq`'s
/// auto-EOI epilogue, wedging the PIC line until reboot.
///
/// If the vector's ISR bit is still set (the task died between the IRQ firing
/// and calling `IrqAck`), a final EOI is sent so the line does not stay
/// masked at the PIC with no subscriber left to acknowledge it.
pub fn release_task(task_id: usize) {
    if task_id == 0 {
        return;
    }

    for (idx, binding) in IRQ_BINDINGS.iter().enumerate() {
        if binding
            .task_id
            .compare_exchange(task_id, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }

        binding.pending.store(false, Ordering::Release);
        wake_all_single(&binding.waitq);

        if is_in_service(idx as u8) {
            end_of_interrupt(idx as u8);
        }
    }
}

/// Resets all IRQ bindings and clears handlers (for unit tests / teardown).
pub fn reset_bindings_for_test() {
    for binding in IRQ_BINDINGS.iter() {
        binding.task_id.store(0, Ordering::Release);
        binding.pending.store(false, Ordering::Release);
        wake_all_single(&binding.waitq);
    }
}
