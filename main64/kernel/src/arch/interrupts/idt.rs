//! Interrupt Descriptor Table (IDT) configuration.

use core::arch::asm;
use core::mem::size_of;

use crate::arch::interrupts::types::{
    EXCEPTION_DEVICE_NOT_AVAILABLE, EXCEPTION_DIVIDE_ERROR, EXCEPTION_DOUBLE_FAULT,
    EXCEPTION_GENERAL_PROTECTION, EXCEPTION_INVALID_OPCODE, EXCEPTION_PAGE_FAULT, IDT_ENTRIES,
    IRQ0_PIT_TIMER_VECTOR, IRQ10_FREE_VECTOR, IRQ11_FREE_VECTOR, IRQ12_PS2_MOUSE_VECTOR,
    IRQ13_FPU_VECTOR, IRQ14_PRIMARY_ATA_VECTOR, IRQ15_SECONDARY_ATA_VECTOR, IRQ1_KEYBOARD_VECTOR,
    IRQ2_PIC_CASCADE_VECTOR, IRQ3_COM2_VECTOR, IRQ4_COM1_VECTOR, IRQ5_LPT2_OR_SOUND_VECTOR,
    IRQ6_FLOPPY_VECTOR, IRQ7_LPT1_OR_SPURIOUS_VECTOR, IRQ8_CMOS_RTC_VECTOR,
    IRQ9_ACPI_OR_LEGACY_VECTOR, SYSCALL_INT80_VECTOR,
};
use crate::arch::interrupts::STATE;

const IDT_PRESENT: u8 = 0x80;
const IDT_INTERRUPT_GATE: u8 = 0x0E;

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
    fn isr7_nm_stub();
    fn isr8_double_fault_stub();
    fn isr13_general_protection_fault_stub();
    fn isr14_page_fault_stub();
    fn int80_syscall_stub();
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    pub offset_low: u16,
    pub selector: u16,
    pub ist: u8,
    pub type_attr: u8,
    pub offset_mid: u16,
    pub offset_high: u32,
    pub zero: u32,
}

impl IdtEntry {
    pub const fn missing() -> Self {
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

    pub fn set_handler(&mut self, handler: usize) {
        self.set_handler_with_dpl(handler, 0);
    }

    pub fn set_handler_with_dpl(&mut self, handler: usize, dpl: u8) {
        self.offset_low = handler as u16;
        self.selector = 0x08;
        self.ist = 0;
        self.type_attr = IDT_PRESENT | IDT_INTERRUPT_GATE | ((dpl & 0x03) << 5);
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.zero = 0;
    }

    /// Configures an IDT gate with explicit privilege level and IST selector.
    ///
    /// Parameters:
    /// - `handler`: 64-bit entry-point address of the interrupt/trap stub.
    /// - `dpl`: Descriptor privilege level (`0..=3`); values above 3 are masked.
    /// - `ist`: Interrupt Stack Table slot (`0..=7`).
    ///
    /// IST semantics in long mode:
    /// - `ist = 0` keeps the current stack (default behavior).
    /// - `ist = 1..=7` instructs the CPU to switch to `TSS.IST[n]` before
    ///   pushing the interrupt frame, which is useful for critical faults
    ///   (e.g., double fault) when the current stack may be corrupted.
    ///
    /// Notes:
    /// - The IST field is 3 bits wide and is therefore masked with `0x07`.
    /// - This function only writes the IDT descriptor; callers must ensure the
    ///   corresponding TSS IST entry is initialized to a valid stack top.
    pub fn set_handler_with_dpl_and_ist(&mut self, handler: usize, dpl: u8, ist: u8) {
        self.set_handler_with_dpl(handler, dpl);
        self.ist = ist & 0x07;
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

pub fn init_idt() {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `STATE.idt` is a singleton table initialized with interrupts disabled.
    // - All vector indices are constants within `0..IDT_ENTRIES`.
    // - `lidt` loads a pointer to this fully initialized IDT image.
    unsafe {
        let idt = &mut *STATE.idt.get();
        idt[EXCEPTION_DIVIDE_ERROR as usize]
            .set_handler(isr0_divide_by_zero_stub as *const () as usize);
        idt[EXCEPTION_INVALID_OPCODE as usize]
            .set_handler(isr6_invalid_opcode_stub as *const () as usize);
        idt[EXCEPTION_DEVICE_NOT_AVAILABLE as usize]
            .set_handler(isr7_nm_stub as *const () as usize);
        // Route double-fault onto IST1 to avoid cascading failures on a broken current stack.
        idt[EXCEPTION_DOUBLE_FAULT as usize].set_handler_with_dpl_and_ist(
            isr8_double_fault_stub as *const () as usize,
            0,
            1,
        );
        idt[EXCEPTION_GENERAL_PROTECTION as usize]
            .set_handler(isr13_general_protection_fault_stub as *const () as usize);
        idt[EXCEPTION_PAGE_FAULT as usize].set_handler(isr14_page_fault_stub as *const () as usize);
        idt[SYSCALL_INT80_VECTOR as usize]
            .set_handler_with_dpl(int80_syscall_stub as *const () as usize, 3);
        idt[IRQ0_PIT_TIMER_VECTOR as usize].set_handler(irq0_pit_timer_stub as *const () as usize);
        idt[IRQ1_KEYBOARD_VECTOR as usize].set_handler(irq1_keyboard_stub as *const () as usize);
        idt[IRQ2_PIC_CASCADE_VECTOR as usize]
            .set_handler(irq2_pic_cascade_stub as *const () as usize);
        idt[IRQ3_COM2_VECTOR as usize].set_handler(irq3_com2_stub as *const () as usize);
        idt[IRQ4_COM1_VECTOR as usize].set_handler(irq4_com1_stub as *const () as usize);
        idt[IRQ5_LPT2_OR_SOUND_VECTOR as usize]
            .set_handler(irq5_lpt2_or_sound_stub as *const () as usize);
        idt[IRQ6_FLOPPY_VECTOR as usize].set_handler(irq6_floppy_stub as *const () as usize);
        idt[IRQ7_LPT1_OR_SPURIOUS_VECTOR as usize]
            .set_handler(irq7_lpt1_or_spurious_stub as *const () as usize);
        idt[IRQ8_CMOS_RTC_VECTOR as usize].set_handler(irq8_cmos_rtc_stub as *const () as usize);
        idt[IRQ9_ACPI_OR_LEGACY_VECTOR as usize]
            .set_handler(irq9_acpi_or_legacy_stub as *const () as usize);
        idt[IRQ10_FREE_VECTOR as usize].set_handler(irq10_free_stub as *const () as usize);
        idt[IRQ11_FREE_VECTOR as usize].set_handler(irq11_free_stub as *const () as usize);
        idt[IRQ12_PS2_MOUSE_VECTOR as usize]
            .set_handler(irq12_ps2_mouse_stub as *const () as usize);
        idt[IRQ13_FPU_VECTOR as usize].set_handler(irq13_fpu_stub as *const () as usize);
        idt[IRQ14_PRIMARY_ATA_VECTOR as usize]
            .set_handler(irq14_primary_ata_stub as *const () as usize);
        idt[IRQ15_SECONDARY_ATA_VECTOR as usize]
            .set_handler(irq15_secondary_ata_stub as *const () as usize);

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

/// Returns the configured IST index for the IDT entry at `vector`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn idt_ist_index(vector: u8) -> u8 {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `STATE.idt` points to the initialized IDT singleton.
    // - Caller provides a vector byte; indexing by `u8` stays within 256 entries.
    unsafe { (&*STATE.idt.get())[vector as usize].ist & 0x07 }
}
