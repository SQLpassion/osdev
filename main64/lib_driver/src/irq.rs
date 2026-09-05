//! Hardware interrupt subscription, waiting, and acknowledgment.

use crate::kernel_types::{decode_result, SysError, SyscallId};
use crate::raw::{syscall1, syscall2};

/// Subscribes this driver task to receive events for the hardware IRQ vector.
pub fn subscribe(vector: u8) -> Result<(), SysError> {
    // SAFETY:
    // - Invokes IrqSubscribe syscall (nr. 32).
    // - Checked by kernel capabilities and resource grants.
    let raw = unsafe { syscall1(SyscallId::IRQ_SUBSCRIBE, vector as u64) };
    decode_result(raw).map(|_| ())
}

/// Blocks until the subscribed hardware IRQ fires or a pending interrupt is serviced.
/// `timeout_ms = 0` means infinite wait.
pub fn wait(vector: u8, timeout_ms: u32) -> Result<(), SysError> {
    // SAFETY:
    // - Invokes IrqWait syscall (nr. 33).
    // - Blocks the task until woken by top-half trampoline or returns immediately if pending.
    let raw = unsafe { syscall2(SyscallId::IRQ_WAIT, vector as u64, timeout_ms as u64) };
    decode_result(raw).map(|_| ())
}

/// Acknowledges handling of an interrupt, sending PIC EOI.
/// Must be called after `wait()` returns and device registers have been serviced.
pub fn ack(vector: u8) -> Result<(), SysError> {
    // SAFETY:
    // - Invokes IrqAck syscall (nr. 34).
    // - Signals completion to the PIC.
    let raw = unsafe { syscall1(SyscallId::IRQ_ACK, vector as u64) };
    decode_result(raw).map(|_| ())
}
