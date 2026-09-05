//! PIC and PIT initialization and helper functions.

use crate::arch::port::PortByte;

#[cfg(debug_assertions)]
use core::sync::atomic::{AtomicU32, Ordering};

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIC_ISR_READ: u8 = 0x0B;

/// OCW2 base for the 8259's "Specific EOI" command. ORing in an IRQ level
/// (0..7, relative to the target PIC) clears exactly that PIC's ISR bit,
/// unlike the "Non-specific EOI" command (`0x20`) which always clears
/// whichever ISR bit is currently highest-priority — the wrong bit whenever
/// more than one IRQ is simultaneously in-service (e.g. two user-space
/// drivers on different lines, one acknowledging while the other is still
/// pending). See `end_of_interrupt`.
const PIC_SPECIFIC_EOI: u8 = 0x60;

const PIC_ICW1_INIT: u8 = 0x10;
const PIC_ICW1_ICW4: u8 = 0x01;
const PIC_ICW4_8086: u8 = 0x01;

const PIT_COMMAND: u16 = 0x43;
const PIT_CHANNEL0: u16 = 0x40;
const PIT_MODE_RATE_GENERATOR: u8 = 0x36;
const PIT_INPUT_HZ: u32 = 1_193_182;

pub fn remap_pic(offset1: u8, offset2: u8) {
    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - PIC command/data ports are valid hardware I/O targets in ring 0.
    // - Sequence follows standard PIC remap initialization protocol.
    unsafe {
        let cmd1 = PortByte::new(PIC1_COMMAND);
        let cmd2 = PortByte::new(PIC2_COMMAND);
        let data1 = PortByte::new(PIC1_DATA);
        let data2 = PortByte::new(PIC2_DATA);

        let icw1 = PIC_ICW1_INIT | PIC_ICW1_ICW4;
        cmd1.write(icw1);
        io_wait();
        cmd2.write(icw1);
        io_wait();

        data1.write(offset1);
        io_wait();
        data2.write(offset2);
        io_wait();

        data1.write(0x04);
        io_wait();
        data2.write(0x02);
        io_wait();

        data1.write(PIC_ICW4_8086);
        io_wait();
        data2.write(PIC_ICW4_8086);
        io_wait();
    }
}

/// Small I/O delay by writing to port 0x80 (POST diagnostic port).
/// This gives the PIC ~1 us to settle between commands, which is
/// necessary on real hardware but harmless on emulators.
#[inline]
pub fn io_wait() {
    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Port `0x80` write is a conventional I/O delay primitive.
    // - No memory dereference or aliasing involved.
    unsafe {
        PortByte::new(0x80).write(0);
    }
}

pub fn mask_pic() {
    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - PIC data ports are valid and writes only adjust IRQ mask state.
    unsafe {
        let data1 = PortByte::new(PIC1_DATA);
        let data2 = PortByte::new(PIC2_DATA);

        // Step 1: Keep timer (IRQ0) and keyboard (IRQ1) enabled.
        // Step 2: Unmask cascade (IRQ2) so slave PIC IRQs can propagate.
        data1.write(0xF8); // Unmask IRQ0 + IRQ1 + IRQ2.

        // Step 3: Unmask primary ATA on slave PIC (IRQ14 -> bit 6).
        // All other slave IRQs remain masked.
        data2.write(0xBF); // 0b1011_1111
    }
}

/// Unmasks a single IRQ line so the 8259 actually forwards it to the CPU.
///
/// `mask_pic()` only unmasks the lines the kernel itself services at boot
/// (timer, keyboard, cascade, ATA); every other line — including any later
/// granted to a Ring-3 driver via `irq_bridge::subscribe()` — stays masked
/// forever unless unmasked here. Without this, a driver's `IrqSubscribe`/
/// `IrqWait` succeed at the syscall level but the hardware interrupt never
/// arrives, since the 8259 itself never forwards it to the CPU.
///
/// `irq` must be in `0..16` (direct IRQ line number, not the IDT vector).
pub fn unmask_irq(irq: u8) {
    debug_assert!(irq < 16, "unmask_irq: irq must be a valid IRQ line (0..16)");

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - PIC data ports are valid and read-modify-write only adjusts IRQ mask state.
    unsafe {
        if irq < 8 {
            let data1 = PortByte::new(PIC1_DATA);
            let mask = data1.read();
            data1.write(mask & !(1 << irq));
        } else {
            let data2 = PortByte::new(PIC2_DATA);
            let mask = data2.read();
            data2.write(mask & !(1 << (irq - 8)));

            // A slave-PIC IRQ can only propagate to the CPU if the master's
            // cascade line (IRQ2) is also unmasked.
            let data1 = PortByte::new(PIC1_DATA);
            let cascade_mask = data1.read();
            data1.write(cascade_mask & !(1 << 2));
        }
    }
}

/// Re-masks a single IRQ line, e.g. once the driver that owned it has exited.
///
/// `irq` must be in `0..16` (direct IRQ line number, not the IDT vector).
pub fn mask_irq(irq: u8) {
    debug_assert!(irq < 16, "mask_irq: irq must be a valid IRQ line (0..16)");

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - PIC data ports are valid and read-modify-write only adjusts IRQ mask state.
    unsafe {
        if irq < 8 {
            let data1 = PortByte::new(PIC1_DATA);
            let mask = data1.read();
            data1.write(mask | (1 << irq));
        } else {
            let data2 = PortByte::new(PIC2_DATA);
            let mask = data2.read();
            data2.write(mask | (1 << (irq - 8)));
        }
    }
}

pub fn end_of_interrupt(irq: u8) {
    // Step 1 (debug/test builds only): count every EOI actually issued to the
    // PIC. Integration tests use this counter to assert that a software
    // `int` on an IRQ vector (e.g. `scheduler::yield_now`) never reaches this
    // function unless the PIC genuinely has that line in-service — see
    // `is_in_service` and its caller in `dispatch_irq` (issue #19).
    #[cfg(debug_assertions)]
    EOI_COUNT.fetch_add(1, Ordering::Relaxed);

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Specific-EOI commands to PIC ports acknowledge exactly `irq`'s ISR bit,
    //   never an unrelated line that happens to be in-service at the same time.
    // - `irq >= 8` correctly determines whether the slave PIC also needs EOI.
    unsafe {
        if irq >= 8 {
            // Clear the slave's own ISR bit for this line.
            PortByte::new(PIC2_COMMAND).write(PIC_SPECIFIC_EOI | (irq - 8));
            // Every slave-originated interrupt also latches the cascade line
            // (IRQ2) on the master; that bit must be EOI'd too, specifically,
            // so an unrelated in-service master IRQ (e.g. IRQ1) is untouched.
            PortByte::new(PIC1_COMMAND).write(PIC_SPECIFIC_EOI | 2);
        } else {
            PortByte::new(PIC1_COMMAND).write(PIC_SPECIFIC_EOI | irq);
        }
    }
}

/// Test-only counter of PIC EOI commands actually issued via
/// [`end_of_interrupt`], across all IRQ lines.
///
/// Not gated behind `#[cfg(test)]`: this crate builds with `[lib] test =
/// false`, and integration tests under `tests/*.rs` link `kaos_kernel` as an
/// ordinary (non-`--cfg test`) dependency, so `#[cfg(test)]` items in `src/`
/// are never visible to them. Gated behind `#[cfg(debug_assertions)]`
/// instead — all test binaries build in the debug profile — matching the
/// existing `scheduler::TEST_SCHEDULER_ENTER_ASSERT_TS_CLEAR` convention.
#[cfg(debug_assertions)]
pub static EOI_COUNT: AtomicU32 = AtomicU32::new(0);

/// Test-only accessor for [`EOI_COUNT`].
///
/// Hidden from public docs; used by integration tests to assert that a given
/// code path did or did not cause a PIC EOI to be issued.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn eoi_count_for_test() -> u32 {
    EOI_COUNT.load(Ordering::Acquire)
}

/// Test-only reset of [`EOI_COUNT`] back to zero.
///
/// Hidden from public docs; lets each test start from a known baseline
/// regardless of EOIs issued by earlier tests or kernel init.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn reset_eoi_count_for_test() {
    EOI_COUNT.store(0, Ordering::Release);
}

/// Computes the PIT divisor for the requested interrupt frequency.
///
/// Returns 0 for `hz == 0` so callers can decide how to handle invalid input.
pub const fn pit_divisor_for_hz(hz: u32) -> u16 {
    if hz == 0 {
        return 0;
    }

    let divisor = PIT_INPUT_HZ / hz;
    if divisor == 0 {
        1
    } else if divisor > u16::MAX as u32 {
        u16::MAX
    } else {
        divisor as u16
    }
}

/// Programs PIT channel 0 as periodic timer with the given frequency.
pub fn init_periodic_timer(hz: u32) {
    let divisor = pit_divisor_for_hz(hz);
    if divisor == 0 {
        return;
    }

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Writing PIT command/data ports is required to program channel 0.
    // - Caller controls when to initialize; this routine only performs I/O port writes.
    unsafe {
        let cmd = PortByte::new(PIT_COMMAND);
        let data = PortByte::new(PIT_CHANNEL0);
        cmd.write(PIT_MODE_RATE_GENERATOR);
        data.write((divisor & 0xFF) as u8);
        data.write((divisor >> 8) as u8);
    }
}

/// Checks whether the given IRQ line is currently marked in-service on the
/// 8259 PIC, i.e. the PIC's In-Service Register (ISR) has the corresponding
/// bit set because a real hardware assertion of that line was acknowledged
/// by the CPU and no EOI has been sent for it yet.
///
/// This is the mechanism that lets [`dispatch_irq`](super::dispatch_irq)
/// distinguish a genuine hardware IRQ entry from a software-triggered `int`
/// on the same vector (e.g. `scheduler::yield_now`'s `int
/// IRQ0_PIT_TIMER_VECTOR`): a software `int` never causes the PIC to latch an
/// ISR bit, because the PIC itself never saw an edge on that line. Sending an
/// EOI for such an entry would be spurious — it acknowledges a service the
/// PIC never recorded, which can desynchronize the PIC's in-service tracking
/// (e.g. by mis-acking a genuinely in-service interrupt on a later, unrelated
/// EOI). See issue #19.
///
/// `irq` must be in `0..16` (direct IRQ line number, not the IDT vector).
pub fn is_in_service(irq: u8) -> bool {
    debug_assert!(
        irq < 16,
        "is_in_service: irq must be a valid IRQ line (0..16)"
    );

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Reading the PIC ISR (In-Service Register) via OCW3 is safe and doesn't mutate PIC state.
    // - `irq < 8` selects the master PIC's own ISR bit; `irq >= 8` selects the
    //   slave PIC's ISR bit for `irq - 8` (the slave enumerates IRQ8..15 as
    //   its own bits 0..7).
    unsafe {
        if irq < 8 {
            let cmd = PortByte::new(PIC1_COMMAND);
            cmd.write(PIC_ISR_READ);
            let isr = cmd.read();
            (isr & (1 << irq)) != 0
        } else {
            let cmd = PortByte::new(PIC2_COMMAND);
            cmd.write(PIC_ISR_READ);
            let isr = cmd.read();
            (isr & (1 << (irq - 8))) != 0
        }
    }
}

/// Checks if the given IRQ is a spurious interrupt from the PIC.
/// Spurious interrupts occur when a device deasserts its IRQ line
/// before the PIC can acknowledge it. The PIC defaults to reporting
/// IRQ 7 (for master) or IRQ 15 (for slave).
pub fn is_spurious_irq(irq: u8) -> bool {
    if irq != 7 && irq != 15 {
        return false;
    }

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Reading the PIC ISR (In-Service Register) is safe and doesn't mutate PIC state.
    unsafe {
        if irq == 7 {
            let cmd = PortByte::new(PIC1_COMMAND);
            cmd.write(PIC_ISR_READ);
            let isr = cmd.read();
            (isr & (1 << 7)) == 0
        } else {
            let cmd = PortByte::new(PIC2_COMMAND);
            cmd.write(PIC_ISR_READ);
            let isr = cmd.read();
            (isr & (1 << 7)) == 0
        }
    }
}
