//! PIC and PIT initialization and helper functions.

use crate::arch::port::PortByte;

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIC_EOI: u8 = 0x20;

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

pub fn end_of_interrupt(irq: u8) {
    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - EOI commands to PIC ports acknowledge serviced IRQ lines.
    // - `irq >= 8` correctly determines whether slave PIC also needs EOI.
    unsafe {
        if irq >= 8 {
            PortByte::new(PIC2_COMMAND).write(PIC_EOI);
        }
        PortByte::new(PIC1_COMMAND).write(PIC_EOI);
    }
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
