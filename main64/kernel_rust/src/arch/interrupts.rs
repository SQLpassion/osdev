//! Interrupt and PIC wiring for Rust-side IRQ handling.

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::mem::size_of;

use crate::arch::port::PortByte;

const IDT_ENTRIES: usize = 256;
const IRQ_BASE: u8 = 32;
pub const IRQ1_VECTOR: u8 = IRQ_BASE + 1;
pub const EXCEPTION_PAGE_FAULT: u8 = 14;

const IDT_PRESENT: u8 = 0x80;
const IDT_INTERRUPT_GATE: u8 = 0x0E;

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;
const PIC_EOI: u8 = 0x20;

const PIC_ICW1_INIT: u8 = 0x10;
const PIC_ICW1_ICW4: u8 = 0x01;
const PIC_ICW4_8086: u8 = 0x01;

global_asm!(
    r#"
    .section .text
    .global irq1_stub
    .type irq1_stub, @function
irq1_stub:
    cli
    push rax
    push rcx
    push rdx
    push rbx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    sub rsp, 8
    mov edi, {irq1_vector}
    call irq1_rust_dispatch
    add rsp, 8

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    pop rdx
    pop rcx
    pop rax
    sti
    iretq

    .global isr14_stub
    .type isr14_stub, @function
isr14_stub:
    push rax
    push rcx
    push rdx
    push rbx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, cr2
    mov rsi, [rsp + 120]
    sub rsp, 8
    call page_fault_handler_rust
    add rsp, 8

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    pop rdx
    pop rcx
    pop rax
    add rsp, 8
    iretq
"#,
    irq1_vector = const IRQ1_VECTOR,
);

extern "C" {
    fn irq1_stub();
    fn isr14_stub();
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            zero: 0,
        }
    }

    fn set_handler(&mut self, handler: usize) {
        self.offset_low = handler as u16;
        self.selector = 0x08;
        self.ist = 0;
        self.type_attr = IDT_PRESENT | IDT_INTERRUPT_GATE;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.zero = 0;
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

type IrqHandler = fn(u8);

/// Holds the IDT and IRQ handler table behind `UnsafeCell` to avoid
/// `static mut` (which permits aliased `&mut` references and is unsound).
struct InterruptState {
    idt: UnsafeCell<[IdtEntry; IDT_ENTRIES]>,
    handlers: UnsafeCell<[Option<IrqHandler>; IDT_ENTRIES]>,
}

impl InterruptState {
    const fn new() -> Self {
        Self {
            idt: UnsafeCell::new([IdtEntry::missing(); IDT_ENTRIES]),
            handlers: UnsafeCell::new([None; IDT_ENTRIES]),
        }
    }
}

// Safety: The kernel is single-threaded (no SMP).  The IDT is written only
// during init() before interrupts are enabled.  IRQ handler slots are written
// with interrupts disabled and read from dispatch_irq in interrupt context;
// no concurrent mutation is possible.
unsafe impl Sync for InterruptState {}

static STATE: InterruptState = InterruptState::new();

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
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Disable interrupts globally.
pub fn disable() {
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

/// Returns whether interrupts are currently enabled (IF flag set).
#[inline]
pub fn are_enabled() -> bool {
    let rflags: u64;
    // SAFETY:
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

fn init_idt() {
    unsafe {
        let idt = &mut *STATE.idt.get();
        idt[EXCEPTION_PAGE_FAULT as usize].set_handler(isr14_stub as *const () as usize);
        idt[IRQ1_VECTOR as usize].set_handler(irq1_stub as *const () as usize);

        let idt_ptr = IdtPointer {
            limit: (size_of::<IdtEntry>() * IDT_ENTRIES - 1) as u16,
            base: STATE.idt.get() as u64,
        };

        asm!(
            "lidt [{}]",
            in(reg) &idt_ptr,
            options(readonly, nostack, preserves_flags)
        );
    }
}

#[no_mangle]
pub extern "C" fn page_fault_handler_rust(faulting_address: u64, error_code: u64) {
    crate::memory::vmm::handle_page_fault(faulting_address, error_code);
}

/// Register a callback for a given interrupt vector.
pub fn register_irq_handler(vector: u8, handler: IrqHandler) {
    unsafe {
        let handlers = &mut *STATE.handlers.get();
        handlers[vector as usize] = Some(handler);
    }
}

fn clear_irq_handlers() {
    unsafe {
        let handlers = &mut *STATE.handlers.get();
        for slot in handlers.iter_mut() {
            *slot = None;
        }
    }
}

fn dispatch_irq(vector: u8) {
    let handler = unsafe {
        let handlers = &*STATE.handlers.get();
        handlers[vector as usize]
    };
    if let Some(handler) = handler {
        handler(vector);
    }

    if (IRQ_BASE..IRQ_BASE + 16).contains(&vector) {
        end_of_interrupt(vector - IRQ_BASE);
    }
}

fn remap_pic(offset1: u8, offset2: u8) {
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
fn io_wait() {
    unsafe {
        PortByte::new(0x80).write(0);
    }
}

fn mask_pic() {
    unsafe {
        let data1 = PortByte::new(PIC1_DATA);
        let data2 = PortByte::new(PIC2_DATA);

        data1.write(0xFD); // Unmask IRQ1 only.
        data2.write(0xFF); // Mask all slave IRQs.
    }
}

fn end_of_interrupt(irq: u8) {
    unsafe {
        if irq >= 8 {
            PortByte::new(PIC2_COMMAND).write(PIC_EOI);
        }
        PortByte::new(PIC1_COMMAND).write(PIC_EOI);
    }
}

/// Dispatch entry point called from the `irq1_stub` assembly trampoline.
///
/// # Safety contract (enforced by caller)
/// - Must be called with interrupts disabled (`cli` before entry).
/// - Must not be called reentrantly — the assembly stub does not
///   re-enable interrupts until after `iretq`.
/// - `vector` must be a valid IRQ vector number (`IRQ_BASE..IRQ_BASE + 16`).
#[no_mangle]
pub extern "C" fn irq1_rust_dispatch(vector: u8) {
    if vector == IRQ1_VECTOR {
        dispatch_irq(vector);
    }
}
