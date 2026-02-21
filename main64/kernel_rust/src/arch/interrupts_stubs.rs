use core::arch::global_asm;

use super::{
    EXCEPTION_DIVIDE_ERROR, EXCEPTION_DOUBLE_FAULT,
    EXCEPTION_GENERAL_PROTECTION, EXCEPTION_INVALID_OPCODE, IRQ0_PIT_TIMER_VECTOR,
    IRQ10_FREE_VECTOR, IRQ11_FREE_VECTOR, IRQ12_PS2_MOUSE_VECTOR, IRQ13_FPU_VECTOR,
    IRQ14_PRIMARY_ATA_VECTOR, IRQ15_SECONDARY_ATA_VECTOR, IRQ1_KEYBOARD_VECTOR,
    IRQ2_PIC_CASCADE_VECTOR, IRQ3_COM2_VECTOR, IRQ4_COM1_VECTOR, IRQ5_LPT2_OR_SOUND_VECTOR,
    IRQ6_FLOPPY_VECTOR, IRQ7_LPT1_OR_SPURIOUS_VECTOR, IRQ8_CMOS_RTC_VECTOR,
    IRQ9_ACPI_OR_LEGACY_VECTOR,
};

/// Number of general-purpose registers saved by IRQ/ISR stubs.
const STUB_SAVED_GPR_COUNT: usize = 15;

/// Stack byte offset from `rsp` to the CPU-pushed exception error code
/// after all general-purpose registers were pushed by the stub.
const STUB_ERROR_CODE_STACK_OFFSET: usize =
    STUB_SAVED_GPR_COUNT * core::mem::size_of::<u64>();

macro_rules! irq_stub_asm {
    ($name:ident, $vector:expr) => {
        global_asm!(
            concat!(
                ".section .text\n",
                ".global ",
                stringify!($name),
                "\n",
                ".type ",
                stringify!($name),
                ", @function\n",
                stringify!($name),
                ":\n",
                "    cli\n",
                "    push rax\n",
                "    push rcx\n",
                "    push rdx\n",
                "    push rbx\n",
                "    push rbp\n",
                "    push rsi\n",
                "    push rdi\n",
                "    push r8\n",
                "    push r9\n",
                "    push r10\n",
                "    push r11\n",
                "    push r12\n",
                "    push r13\n",
                "    push r14\n",
                "    push r15\n",
                "    mov edi, {vector}\n",
                "    mov rsi, rsp\n",
                "    and rsp, -16\n",
                "    call irq_rust_dispatch\n",
                "    mov rsp, rax\n",
                "    pop r15\n",
                "    pop r14\n",
                "    pop r13\n",
                "    pop r12\n",
                "    pop r11\n",
                "    pop r10\n",
                "    pop r9\n",
                "    pop r8\n",
                "    pop rdi\n",
                "    pop rsi\n",
                "    pop rbp\n",
                "    pop rbx\n",
                "    pop rdx\n",
                "    pop rcx\n",
                "    pop rax\n",
                "    iretq\n",
            ),
            vector = const $vector,
        );
    };
}

macro_rules! isr_stub_without_error_code_asm {
    ($name:ident, $vector:expr) => {
        global_asm!(
            concat!(
                ".section .text\n",
                ".global ",
                stringify!($name),
                "\n",
                ".type ",
                stringify!($name),
                ", @function\n",
                stringify!($name),
                ":\n",
                "    cli\n",
                "    push rax\n",
                "    push rcx\n",
                "    push rdx\n",
                "    push rbx\n",
                "    push rbp\n",
                "    push rsi\n",
                "    push rdi\n",
                "    push r8\n",
                "    push r9\n",
                "    push r10\n",
                "    push r11\n",
                "    push r12\n",
                "    push r13\n",
                "    push r14\n",
                "    push r15\n",
                "    mov edi, {vector}\n",
                "    xor esi, esi\n",
                "    mov rdx, rsp\n",
                "    and rsp, -16\n",
                "    call exception_handler_rust\n",
                "1:\n",
                "    cli\n",
                "    hlt\n",
                "    jmp 1b\n",
            ),
            vector = const $vector,
        );
    };
}

macro_rules! isr_stub_with_error_code_asm {
    ($name:ident, $vector:expr) => {
        global_asm!(
            concat!(
                ".section .text\n",
                ".global ",
                stringify!($name),
                "\n",
                ".type ",
                stringify!($name),
                ", @function\n",
                stringify!($name),
                ":\n",
                "    cli\n",
                "    push rax\n",
                "    push rcx\n",
                "    push rdx\n",
                "    push rbx\n",
                "    push rbp\n",
                "    push rsi\n",
                "    push rdi\n",
                "    push r8\n",
                "    push r9\n",
                "    push r10\n",
                "    push r11\n",
                "    push r12\n",
                "    push r13\n",
                "    push r14\n",
                "    push r15\n",
                "    mov edi, {vector}\n",
                "    mov rsi, [rsp + {error_code_stack_offset}]\n",
                "    mov rdx, rsp\n",
                "    and rsp, -16\n",
                "    call exception_handler_rust\n",
                "1:\n",
                "    cli\n",
                "    hlt\n",
                "    jmp 1b\n",
            ),
            vector = const $vector,
            error_code_stack_offset = const STUB_ERROR_CODE_STACK_OFFSET,
        );
    };
}

irq_stub_asm!(irq0_pit_timer_stub, IRQ0_PIT_TIMER_VECTOR);
irq_stub_asm!(irq1_keyboard_stub, IRQ1_KEYBOARD_VECTOR);
irq_stub_asm!(irq2_pic_cascade_stub, IRQ2_PIC_CASCADE_VECTOR);
irq_stub_asm!(irq3_com2_stub, IRQ3_COM2_VECTOR);
irq_stub_asm!(irq4_com1_stub, IRQ4_COM1_VECTOR);
irq_stub_asm!(irq5_lpt2_or_sound_stub, IRQ5_LPT2_OR_SOUND_VECTOR);
irq_stub_asm!(irq6_floppy_stub, IRQ6_FLOPPY_VECTOR);
irq_stub_asm!(irq7_lpt1_or_spurious_stub, IRQ7_LPT1_OR_SPURIOUS_VECTOR);
irq_stub_asm!(irq8_cmos_rtc_stub, IRQ8_CMOS_RTC_VECTOR);
irq_stub_asm!(irq9_acpi_or_legacy_stub, IRQ9_ACPI_OR_LEGACY_VECTOR);
irq_stub_asm!(irq10_free_stub, IRQ10_FREE_VECTOR);
irq_stub_asm!(irq11_free_stub, IRQ11_FREE_VECTOR);
irq_stub_asm!(irq12_ps2_mouse_stub, IRQ12_PS2_MOUSE_VECTOR);
irq_stub_asm!(irq13_fpu_stub, IRQ13_FPU_VECTOR);
irq_stub_asm!(irq14_primary_ata_stub, IRQ14_PRIMARY_ATA_VECTOR);
irq_stub_asm!(irq15_secondary_ata_stub, IRQ15_SECONDARY_ATA_VECTOR);

isr_stub_without_error_code_asm!(isr0_divide_by_zero_stub, EXCEPTION_DIVIDE_ERROR);
isr_stub_without_error_code_asm!(isr6_invalid_opcode_stub, EXCEPTION_INVALID_OPCODE);

// Vector 7 (#NM — Device Not Available) is handled by the lazy FPU switcher.
// Unlike other exception stubs this one must *return* (via iretq) so that
// the CPU can re-execute the faulting FPU/SSE instruction after the handler
// has restored the task's FPU state and cleared CR0.TS.
//
// Stack layout on entry (from the CPU's perspective, growing downward):
//   [RSP+40]  SS
//   [RSP+32]  RSP (user/kernel)
//   [RSP+24]  RFLAGS
//   [RSP+16]  CS
//   [RSP+ 8]  RIP  (address of the faulting FPU instruction)
//   [RSP+ 0]  ← exception entry point (no error code for #NM)
//
// The stub saves all 15 caller-/callee-saved GPRs, calls nm_rust_handler
// (which clears CR0.TS and restores the task's FPU state), then restores the
// GPRs and executes iretq.  No EOI is sent to the PIC because #NM is a
// CPU exception, not a hardware IRQ.
global_asm!(
    r#"
    .section .text
    .global isr7_nm_stub
    .type isr7_nm_stub, @function
isr7_nm_stub:
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
    call nm_rust_handler
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
    iretq
"#,
);
isr_stub_with_error_code_asm!(isr8_double_fault_stub, EXCEPTION_DOUBLE_FAULT);
isr_stub_with_error_code_asm!(
    isr13_general_protection_fault_stub,
    EXCEPTION_GENERAL_PROTECTION
);

global_asm!(
    r#"
    .section .text
    .global isr14_page_fault_stub
    .type isr14_page_fault_stub, @function
isr14_page_fault_stub:
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
    mov rsi, [rsp + {error_code_stack_offset}]
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
    error_code_stack_offset = const STUB_ERROR_CODE_STACK_OFFSET,
);

global_asm!(
    r#"
    .section .text
    .global int80_syscall_stub
    .type int80_syscall_stub, @function
int80_syscall_stub:
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

    mov rdi, rsp
    and rsp, -16
    call syscall_rust_dispatch
    mov rsp, rax

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
    iretq
"#,
);
