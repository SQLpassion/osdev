//! Interrupt and PIC wiring for Rust-side IRQ handling.

use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::size_of;

use crate::arch::port::PortByte;

const IDT_ENTRIES: usize = 256;
const IRQ_BASE: u8 = 32;
pub const IRQ0_PIT_TIMER_VECTOR: u8 = IRQ_BASE;
pub const IRQ1_KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;
const IRQ2_PIC_CASCADE_VECTOR: u8 = IRQ_BASE + 2;
const IRQ3_COM2_VECTOR: u8 = IRQ_BASE + 3;
const IRQ4_COM1_VECTOR: u8 = IRQ_BASE + 4;
const IRQ5_LPT2_OR_SOUND_VECTOR: u8 = IRQ_BASE + 5;
const IRQ6_FLOPPY_VECTOR: u8 = IRQ_BASE + 6;
const IRQ7_LPT1_OR_SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;
const IRQ8_CMOS_RTC_VECTOR: u8 = IRQ_BASE + 8;
const IRQ9_ACPI_OR_LEGACY_VECTOR: u8 = IRQ_BASE + 9;
const IRQ10_FREE_VECTOR: u8 = IRQ_BASE + 10;
const IRQ11_FREE_VECTOR: u8 = IRQ_BASE + 11;
const IRQ12_PS2_MOUSE_VECTOR: u8 = IRQ_BASE + 12;
const IRQ13_FPU_VECTOR: u8 = IRQ_BASE + 13;
const IRQ14_PRIMARY_ATA_VECTOR: u8 = IRQ_BASE + 14;
const IRQ15_SECONDARY_ATA_VECTOR: u8 = IRQ_BASE + 15;
pub const SYSCALL_INT80_VECTOR: u8 = 0x80;
pub const EXCEPTION_DIVIDE_ERROR: u8 = 0;
pub const EXCEPTION_INVALID_OPCODE: u8 = 6;
pub const EXCEPTION_DEVICE_NOT_AVAILABLE: u8 = 7;
pub const EXCEPTION_DOUBLE_FAULT: u8 = 8;
pub const EXCEPTION_GENERAL_PROTECTION: u8 = 13;
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

const PIT_COMMAND: u16 = 0x43;
const PIT_CHANNEL0: u16 = 0x40;
const PIT_MODE_RATE_GENERATOR: u8 = 0x36;
const PIT_INPUT_HZ: u32 = 1_193_182;
const VGA_TEXT_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;

/// Saved general-purpose register state as pushed by the IRQ trampolines.
///
/// Layout contract:
/// - Must match the push/pop order in all generated IRQ stubs.
/// - Any change requires synchronized updates in assembly and tests.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SavedRegisters {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
}

/// Hardware interrupt return frame for `iretq` in 64-bit long mode.
///
/// Layout contract:
/// - In IA-32e mode, `iretq` **unconditionally** pops all five values
///   (RIP, CS, RFLAGS, RSP, SS), regardless of privilege-level change.
/// - Must match the push order used by the CPU on interrupt entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InterruptStackFrame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

#[path = "interrupts_stubs.rs"]
mod interrupts_stubs;

extern "C" {
    fn irq0_pit_timer_stub();
    fn irq1_keyboard_stub();
    fn irq2_pic_cascade_stub();
    fn irq3_com2_stub();
    fn irq4_com1_stub();
    fn irq5_lpt2_or_sound_stub();
    fn irq6_floppy_stub();
    fn irq7_lpt1_or_spurious_stub();
    fn irq8_cmos_rtc_stub();
    fn irq9_acpi_or_legacy_stub();
    fn irq10_free_stub();
    fn irq11_free_stub();
    fn irq12_ps2_mouse_stub();
    fn irq13_fpu_stub();
    fn irq14_primary_ata_stub();
    fn irq15_secondary_ata_stub();
    fn isr0_divide_by_zero_stub();
    fn isr6_invalid_opcode_stub();
    fn isr7_device_not_available_stub();
    fn isr8_double_fault_stub();
    fn isr13_general_protection_fault_stub();
    fn isr14_page_fault_stub();
    fn int80_syscall_stub();
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
        self.set_handler_with_dpl(handler, 0);
    }

    fn set_handler_with_dpl(&mut self, handler: usize, dpl: u8) {
        self.offset_low = handler as u16;
        self.selector = 0x08;
        self.ist = 0;
        self.type_attr = IDT_PRESENT | IDT_INTERRUPT_GATE | ((dpl & 0x03) << 5);
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

type IrqHandler = fn(u8, &mut SavedRegisters) -> *mut SavedRegisters;

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
        idt[EXCEPTION_DIVIDE_ERROR as usize].set_handler(isr0_divide_by_zero_stub as *const () as usize);
        idt[EXCEPTION_INVALID_OPCODE as usize].set_handler(isr6_invalid_opcode_stub as *const () as usize);
        idt[EXCEPTION_DEVICE_NOT_AVAILABLE as usize]
            .set_handler(isr7_device_not_available_stub as *const () as usize);
        idt[EXCEPTION_DOUBLE_FAULT as usize].set_handler(isr8_double_fault_stub as *const () as usize);
        idt[EXCEPTION_GENERAL_PROTECTION as usize]
            .set_handler(isr13_general_protection_fault_stub as *const () as usize);
        idt[EXCEPTION_PAGE_FAULT as usize].set_handler(isr14_page_fault_stub as *const () as usize);
        idt[SYSCALL_INT80_VECTOR as usize]
            .set_handler_with_dpl(int80_syscall_stub as *const () as usize, 3);
        idt[IRQ0_PIT_TIMER_VECTOR as usize].set_handler(irq0_pit_timer_stub as *const () as usize);
        idt[IRQ1_KEYBOARD_VECTOR as usize].set_handler(irq1_keyboard_stub as *const () as usize);
        idt[IRQ2_PIC_CASCADE_VECTOR as usize].set_handler(irq2_pic_cascade_stub as *const () as usize);
        idt[IRQ3_COM2_VECTOR as usize].set_handler(irq3_com2_stub as *const () as usize);
        idt[IRQ4_COM1_VECTOR as usize].set_handler(irq4_com1_stub as *const () as usize);
        idt[IRQ5_LPT2_OR_SOUND_VECTOR as usize].set_handler(irq5_lpt2_or_sound_stub as *const () as usize);
        idt[IRQ6_FLOPPY_VECTOR as usize].set_handler(irq6_floppy_stub as *const () as usize);
        idt[IRQ7_LPT1_OR_SPURIOUS_VECTOR as usize].set_handler(irq7_lpt1_or_spurious_stub as *const () as usize);
        idt[IRQ8_CMOS_RTC_VECTOR as usize].set_handler(irq8_cmos_rtc_stub as *const () as usize);
        idt[IRQ9_ACPI_OR_LEGACY_VECTOR as usize].set_handler(irq9_acpi_or_legacy_stub as *const () as usize);
        idt[IRQ10_FREE_VECTOR as usize].set_handler(irq10_free_stub as *const () as usize);
        idt[IRQ11_FREE_VECTOR as usize].set_handler(irq11_free_stub as *const () as usize);
        idt[IRQ12_PS2_MOUSE_VECTOR as usize].set_handler(irq12_ps2_mouse_stub as *const () as usize);
        idt[IRQ13_FPU_VECTOR as usize].set_handler(irq13_fpu_stub as *const () as usize);
        idt[IRQ14_PRIMARY_ATA_VECTOR as usize].set_handler(irq14_primary_ata_stub as *const () as usize);
        idt[IRQ15_SECONDARY_ATA_VECTOR as usize].set_handler(irq15_secondary_ata_stub as *const () as usize);

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

/// Returns whether a CPU exception vector pushes an error code on entry.
pub const fn exception_has_error_code(vector: u8) -> bool {
    matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
}

#[inline]
const fn hex_nibble_ascii(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + (nibble - 10)
    }
}

fn write_exception_banner(vector: u8, error_code: u64, frame: *const SavedRegisters) {
    let mut line = [b' '; VGA_COLS];
    line[0] = b'!';
    line[1] = b'!';
    line[2] = b' ';
    line[3] = b'E';
    line[4] = b'X';
    line[5] = b'C';
    line[6] = b' ';
    line[7] = b'v';
    line[8] = b'e';
    line[9] = b'c';
    line[10] = b'=';
    line[11] = hex_nibble_ascii((vector >> 4) & 0x0F);
    line[12] = hex_nibble_ascii(vector & 0x0F);
    line[13] = b' ';
    line[14] = b'e';
    line[15] = b'r';
    line[16] = b'r';
    line[17] = b'=';
    for i in 0..16 {
        let shift = (15 - i) * 4;
        line[18 + i] = hex_nibble_ascii(((error_code >> shift) & 0x0F) as u8);
    }
    line[34] = b' ';
    line[35] = b'f';
    line[36] = b'r';
    line[37] = b'm';
    line[38] = b'=';
    let frame_u64 = frame as u64;
    for i in 0..16 {
        let shift = (15 - i) * 4;
        line[39 + i] = hex_nibble_ascii(((frame_u64 >> shift) & 0x0F) as u8);
    }

    // SAFETY:
    // - VGA text memory is MMIO-mapped at `VGA_TEXT_BUFFER`.
    // - We only write one in-bounds row (0..80 cells).
    // - Volatile writes are required for MMIO ordering/visibility.
    unsafe {
        for (col, ch) in line.iter().enumerate() {
            let cell = VGA_TEXT_BUFFER + col * 2;
            core::ptr::write_volatile(cell as *mut u8, *ch);
            core::ptr::write_volatile((cell + 1) as *mut u8, 0x4F);
        }
    }
}

/// Fatal exception sink for vectors with dedicated stubs.
///
/// Called from assembly stubs for faults we currently treat as unrecoverable.
#[no_mangle]
pub extern "C" fn exception_handler_rust(vector: u8, error_code: u64, frame: *const SavedRegisters) -> ! {
    let has_error_code = exception_has_error_code(vector);
    let iret_ptr = (frame as usize)
        + size_of::<SavedRegisters>()
        + if has_error_code { size_of::<u64>() } else { 0 };
    let iret_frame = unsafe {
        // SAFETY:
        // - `frame` points at the register-save area pushed by the ISR stub.
        // - The CPU-pushed interrupt return frame immediately follows saved regs
        //   (plus optional error code for vectors that carry one).
        &*(iret_ptr as *const InterruptStackFrame)
    };
    crate::drivers::serial::_debug_print(format_args!(
        "FATAL EXCEPTION vec=0x{:02x} has_err={} err=0x{:016x} frame=0x{:016x} rip=0x{:016x} cs=0x{:016x} rflags=0x{:016x}\n",
        vector,
        has_error_code,
        error_code,
        frame as u64,
        iret_frame.rip,
        iret_frame.cs,
        iret_frame.rflags
    ));
    write_exception_banner(vector, error_code, frame);

    loop {
        // SAFETY:
        // - We are in a fatal exception path and intentionally stop forward progress.
        // - `cli; hlt` is the standard terminal halt sequence for kernel panic/fault sinks.
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack, preserves_flags));
        }
    }
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

fn dispatch_irq(vector: u8, frame: &mut SavedRegisters) -> *mut SavedRegisters {
    let handler = unsafe {
        let handlers = &*STATE.handlers.get();
        handlers[vector as usize]
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

        data1.write(0xFC); // Unmask IRQ0 + IRQ1.
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

/// Dispatch entry point called from the IRQ assembly trampoline.
///
/// # Safety
/// - Must be called with interrupts disabled (`cli` before entry).
/// - Must not be called reentrantly — the assembly stub does not
///   re-enable interrupts until after `iretq`.
/// - `vector` must be a valid IRQ vector number (`IRQ_BASE..IRQ_BASE + 16`).
#[no_mangle]
pub unsafe extern "C" fn irq_rust_dispatch(vector: u8, frame: *mut SavedRegisters) -> *mut SavedRegisters {
    if !(IRQ_BASE..IRQ_BASE + 16).contains(&vector) {
        return frame;
    }

    let frame = unsafe {
        // SAFETY:
        // - `frame` is provided by the IRQ assembly stubs and points to the
        //   register save area currently living on the active kernel stack.
        // - It remains valid until the stub restores registers and executes `iretq`.
        &mut *frame
    };

    dispatch_irq(vector, frame)
}

/// Dispatch entry point for software interrupt `int 0x80`.
///
/// # Safety
/// - Must be entered only from the dedicated `int80_syscall_stub`.
/// - `frame` must point to the saved-register frame on the active kernel stack.
#[no_mangle]
pub unsafe extern "C" fn syscall_rust_dispatch(frame: *mut SavedRegisters) -> *mut SavedRegisters {
    let frame = unsafe {
        // SAFETY:
        // - `frame` is provided by the syscall assembly stub.
        // - It points at a live register-save area until the stub restores regs and returns via `iretq`.
        &mut *frame
    };

    let syscall_nr = frame.rax;
    let arg0 = frame.rdi;
    let arg1 = frame.rsi;
    let arg2 = frame.rdx;
    let arg3 = frame.r10;
    let result = crate::syscall::dispatch(syscall_nr, arg0, arg1, arg2, arg3);
    frame.rax = result;
    frame as *mut SavedRegisters
}

const _: () = {
    assert!(size_of::<SavedRegisters>() == 15 * 8);
};

const _: () = {
    assert!(size_of::<InterruptStackFrame>() == 5 * 8);
};
