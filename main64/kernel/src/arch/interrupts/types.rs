//! Saved registers, stack frames, and vectors.

pub const IDT_ENTRIES: usize = 256;
pub const IRQ_BASE: u8 = 32;
pub const IRQ_LINES: usize = 16;
pub const IRQ0_PIT_TIMER_VECTOR: u8 = IRQ_BASE;
pub const IRQ1_KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;
pub const IRQ2_PIC_CASCADE_VECTOR: u8 = IRQ_BASE + 2;
pub const IRQ3_COM2_VECTOR: u8 = IRQ_BASE + 3;
pub const IRQ4_COM1_VECTOR: u8 = IRQ_BASE + 4;
pub const IRQ5_LPT2_OR_SOUND_VECTOR: u8 = IRQ_BASE + 5;
pub const IRQ6_FLOPPY_VECTOR: u8 = IRQ_BASE + 6;
pub const IRQ7_LPT1_OR_SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;
pub const IRQ8_CMOS_RTC_VECTOR: u8 = IRQ_BASE + 8;
pub const IRQ9_ACPI_OR_LEGACY_VECTOR: u8 = IRQ_BASE + 9;
pub const IRQ10_FREE_VECTOR: u8 = IRQ_BASE + 10;
pub const IRQ11_FREE_VECTOR: u8 = IRQ_BASE + 11;
pub const IRQ12_PS2_MOUSE_VECTOR: u8 = IRQ_BASE + 12;
pub const IRQ13_FPU_VECTOR: u8 = IRQ_BASE + 13;
pub const IRQ14_PRIMARY_ATA_VECTOR: u8 = IRQ_BASE + 14;
pub const IRQ15_SECONDARY_ATA_VECTOR: u8 = IRQ_BASE + 15;
pub const SYSCALL_INT80_VECTOR: u8 = 0x80;
pub const EXCEPTION_DIVIDE_ERROR: u8 = 0;
pub const EXCEPTION_INVALID_OPCODE: u8 = 6;
pub const EXCEPTION_DEVICE_NOT_AVAILABLE: u8 = 7;
pub const EXCEPTION_DOUBLE_FAULT: u8 = 8;
pub const EXCEPTION_GENERAL_PROTECTION: u8 = 13;
pub const EXCEPTION_PAGE_FAULT: u8 = 14;

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

pub type IrqHandler = fn(u8, &mut SavedRegisters) -> *mut SavedRegisters;
