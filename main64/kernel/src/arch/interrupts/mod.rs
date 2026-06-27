//! Interrupt and PIC wiring for Rust-side IRQ handling on x86_64.
//!
//! Design summary:
//! - Configures the Interrupt Descriptor Table (IDT) with 256 interrupt gates.
//! - Remaps dual 8259 Programmable Interrupt Controllers (PIC) to vector base 32.
//! - Registers assembly entry stubs for exception handling, hardware IRQs, and syscalls.
//! - Integrates the Interrupt Stack Table (IST1) for the Double Fault exception
//!   to recover safely from stack overflow scenarios.
//! - Manages thread-safe register/unregister routines for IRQ callback handlers.
//! - Services system timer ticks by programming PIT channel 0 at a configurable frequency.
//! - Routes `int 0x80` software interrupts to the system call dispatcher.
//!
//! Notes:
//! - Interrupt handlers run with CPU interrupts disabled.
//! - Interrupted CPU context registers are saved in a `SavedRegisters` frame on the stack.
//! - The global interrupt registration state is managed inside a thread-safe `InterruptState`.

use core::arch::asm;
use core::cell::UnsafeCell;

pub mod handlers;
pub mod idt;
pub mod pic;
mod stubs;
pub mod types;

#[allow(unused_imports)]
pub use types::{
    InterruptStackFrame, IrqHandler, SavedRegisters, EXCEPTION_DEVICE_NOT_AVAILABLE,
    EXCEPTION_DIVIDE_ERROR, EXCEPTION_DOUBLE_FAULT, EXCEPTION_GENERAL_PROTECTION,
    EXCEPTION_INVALID_OPCODE, EXCEPTION_PAGE_FAULT, IRQ0_PIT_TIMER_VECTOR,
    IRQ14_PRIMARY_ATA_VECTOR, IRQ1_KEYBOARD_VECTOR, SYSCALL_INT80_VECTOR,
};

#[allow(unused_imports)]
pub use pic::{
    end_of_interrupt, init_periodic_timer, io_wait, mask_pic, pit_divisor_for_hz, remap_pic,
};

#[allow(unused_imports)]
pub use idt::{idt_ist_index, init_idt};

#[allow(unused_imports)]
pub use handlers::{
    exception_handler_rust, exception_has_error_code, irq_rust_dispatch, nm_rust_handler,
    page_fault_handler_rust, syscall_rust_dispatch,
};

use types::{IDT_ENTRIES, IRQ_BASE, IRQ_LINES};

/// Holds the IDT and IRQ handler table behind `UnsafeCell` to avoid
/// `static mut` (which permits aliased `&mut` references and is unsound).
pub struct InterruptState {
    pub idt: UnsafeCell<[idt::IdtEntry; IDT_ENTRIES]>,
    pub irq_handlers: UnsafeCell<[Option<IrqHandler>; IRQ_LINES]>,
}

impl InterruptState {
    const fn new() -> Self {
        Self {
            idt: UnsafeCell::new([idt::IdtEntry::missing(); IDT_ENTRIES]),
            irq_handlers: UnsafeCell::new([None; IRQ_LINES]),
        }
    }
}

// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - The kernel is single-threaded (no SMP).
// - The IDT is written only during `init_idt()` before interrupts are enabled and never mutated afterward.
// - IRQ handler slots are written by `register_irq_handler`, which always disables
//   interrupts for the duration of the write (enforced internally, not by callers).
// - `dispatch_irq` reads the handler table only from IRQ context, where the CPU
//   has already cleared the interrupt flag. No concurrent mutation is possible.
unsafe impl Sync for InterruptState {}

pub(crate) static STATE: InterruptState = InterruptState::new();

/// Initialize IDT and PIC for IRQ handling.
pub fn init() {
    disable();
    init_idt();
    remap_pic(IRQ_BASE, IRQ_BASE + 8);
    mask_pic();
    clear_irq_handlers();
}

/// Enable interrupts globally.
pub fn enable() {
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - `sti` is privileged and valid in ring 0.
    // - Only CPU interrupt flag is modified.
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Disable interrupts globally.
pub fn disable() {
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - `cli` is privileged and valid in ring 0.
    // - Only CPU interrupt flag is modified.
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

/// Returns whether interrupts are currently enabled (IF flag set).
#[inline]
pub fn are_enabled() -> bool {
    let rflags: u64;
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Reading RFLAGS via pushfq/pop is safe and does not modify flags.
    // - `rflags` is a plain register output.
    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
    }
    (rflags & (1 << 9)) != 0
}

/// Register a callback for a given interrupt vector.
///
/// Safe to call at any time, including after interrupts have been globally
/// enabled. Interrupts are briefly disabled for the duration of the table
/// write so that `dispatch_irq` cannot observe a partially-written handler
/// slot when an IRQ fires between a read-check and the store.
pub fn register_irq_handler(vector: u8, handler: IrqHandler) {
    // Step 1: only hardware IRQ vectors are valid for this registry API.
    let irq_idx = irq_slot_index(vector).expect("register_irq_handler: vector is not a PIC IRQ");

    // Step 2: disable interrupts for the duration of the table write.
    // `dispatch_irq` reads the same slot while in IRQ context (CPU already
    // has interrupts off). Disabling here ensures the write is never
    // preempted by an IRQ that reads a half-written slot.
    let interrupts_were_enabled = are_enabled();
    if interrupts_were_enabled {
        disable();
    }

    // Step 3: store handler in direct IRQ-indexed dispatch table.
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Handler table is a singleton owned by this module.
    // - `irq_idx` is derived from validated IRQ range and is in-bounds for `IRQ_LINES`.
    // - Interrupts are disabled for the duration of this write (see above).
    unsafe {
        let handlers = &mut *STATE.irq_handlers.get();
        handlers[irq_idx] = Some(handler);
    }

    // Step 4: restore the caller's interrupt state.
    if interrupts_were_enabled {
        enable();
    }
}

fn clear_irq_handlers() {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Called during init with interrupts disabled.
    // - Mutably accesses the singleton handler table.
    unsafe {
        let handlers = &mut *STATE.irq_handlers.get();

        for slot in handlers.iter_mut() {
            *slot = None;
        }
    }
}

pub(crate) fn dispatch_irq(vector: u8, frame: &mut SavedRegisters) -> *mut SavedRegisters {
    // Step 1: map vector to direct IRQ slot (irq0..irq15 => 0..15).
    let Some(irq_idx) = irq_slot_index(vector) else {
        return frame as *mut SavedRegisters;
    };

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Reads the immutable snapshot of singleton handler table.
    // - `irq_idx` is validated and in-bounds for `IRQ_LINES`.
    let handler = unsafe {
        let handlers = &*STATE.irq_handlers.get();
        handlers[irq_idx]
    };

    let mut next_frame = frame as *mut SavedRegisters;

    if let Some(handler) = handler {
        next_frame = handler(vector, frame);
    }

    if (IRQ_BASE..IRQ_BASE + 16).contains(&vector) {
        end_of_interrupt(vector - IRQ_BASE);
    }

    next_frame
}

#[inline]
fn irq_slot_index(vector: u8) -> Option<usize> {
    // Convert PIC vector space (`IRQ_BASE..IRQ_BASE+16`) into direct slot index.
    if !(IRQ_BASE..IRQ_BASE + IRQ_LINES as u8).contains(&vector) {
        return None;
    }

    Some((vector - IRQ_BASE) as usize)
}

const _: () = {
    assert!(core::mem::size_of::<SavedRegisters>() == 15 * 8);
};

const _: () = {
    assert!(core::mem::size_of::<InterruptStackFrame>() == 5 * 8);
};
