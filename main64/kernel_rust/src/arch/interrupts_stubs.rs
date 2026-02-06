use core::arch::global_asm;

use super::{
    EXCEPTION_DEVICE_NOT_AVAILABLE, EXCEPTION_DIVIDE_ERROR, EXCEPTION_DOUBLE_FAULT,
    EXCEPTION_GENERAL_PROTECTION, EXCEPTION_INVALID_OPCODE, IRQ0_VECTOR, IRQ10_VECTOR,
    IRQ11_VECTOR, IRQ12_VECTOR, IRQ13_VECTOR, IRQ14_VECTOR, IRQ15_VECTOR, IRQ1_VECTOR,
    IRQ2_VECTOR, IRQ3_VECTOR, IRQ4_VECTOR, IRQ5_VECTOR, IRQ6_VECTOR, IRQ7_VECTOR, IRQ8_VECTOR,
    IRQ9_VECTOR,
};

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
                "    mov rsi, [rsp + 120]\n",
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

irq_stub_asm!(irq0_pit_timer_stub, IRQ0_VECTOR);
irq_stub_asm!(irq1_keyboard_stub, IRQ1_VECTOR);
irq_stub_asm!(irq2_pic_cascade_stub, IRQ2_VECTOR);
irq_stub_asm!(irq3_com2_stub, IRQ3_VECTOR);
irq_stub_asm!(irq4_com1_stub, IRQ4_VECTOR);
irq_stub_asm!(irq5_lpt2_or_sound_stub, IRQ5_VECTOR);
irq_stub_asm!(irq6_floppy_stub, IRQ6_VECTOR);
irq_stub_asm!(irq7_lpt1_or_spurious_stub, IRQ7_VECTOR);
irq_stub_asm!(irq8_cmos_rtc_stub, IRQ8_VECTOR);
irq_stub_asm!(irq9_acpi_or_legacy_stub, IRQ9_VECTOR);
irq_stub_asm!(irq10_free_stub, IRQ10_VECTOR);
irq_stub_asm!(irq11_free_stub, IRQ11_VECTOR);
irq_stub_asm!(irq12_ps2_mouse_stub, IRQ12_VECTOR);
irq_stub_asm!(irq13_fpu_stub, IRQ13_VECTOR);
irq_stub_asm!(irq14_primary_ata_stub, IRQ14_VECTOR);
irq_stub_asm!(irq15_secondary_ata_stub, IRQ15_VECTOR);

isr_stub_without_error_code_asm!(isr0_divide_by_zero_stub, EXCEPTION_DIVIDE_ERROR);
isr_stub_without_error_code_asm!(isr6_invalid_opcode_stub, EXCEPTION_INVALID_OPCODE);
isr_stub_without_error_code_asm!(isr7_device_not_available_stub, EXCEPTION_DEVICE_NOT_AVAILABLE);
isr_stub_with_error_code_asm!(isr8_double_fault_stub, EXCEPTION_DOUBLE_FAULT);
isr_stub_with_error_code_asm!(isr13_general_protection_fault_stub, EXCEPTION_GENERAL_PROTECTION);

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
);
