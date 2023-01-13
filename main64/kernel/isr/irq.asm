[BITS 64]
[EXTERN IrqHandler]
[EXTERN MoveToNextTask] 

%MACRO IRQ 2
    GLOBAL Irq%1
    Irq%1:
        ; Disable interrupts
        CLI

        ; Save the General Purpose Registers on the Stack
        PUSH    RDI
        PUSH    RSI
        PUSH    RBP
        PUSH    RSP
        PUSH    RAX
        PUSH    RBX
        PUSH    RCX
        PUSH    RDX
        PUSH    R8
        PUSH    R9
        PUSH    R10
        PUSH    R11
        PUSH    R12
        PUSH    R13
        PUSH    R14
        PUSH    R15

        ; Call the ISR handler that is implemented in C
        MOV     RDI, %2
        CALL    IrqHandler

        ; Restore the General Purpose Registers from the Stack
        POP     R15
        POP     R14
        POP     R13
        POP     R12
        POP     R11
        POP     R10
        POP     R9
        POP     R8
        POP     RDX
        POP     RCX
        POP     RBX
        POP     RAX
        POP     RSP
        POP     RBP
        POP     RSI
        POP     RDI

        ; Enable Interrupts
        STI

        ; Return
        IRETQ
%ENDMACRO

IRQ 0,  32  ; Timer
IRQ 1,  33  ; Keyboard
IRQ 2,  34  ; Cascade for 8259A Slave Controller
IRQ 3,  35  ; Serial Port 2
IRQ 4,  36  ; Serial Port 1
IRQ 5,  37  ; AT systems: Parallel Port 2. PS/2 systems: Reserved
IRQ 6,  38  ; Floppy Drive
IRQ 7,  39  ; Parallel Port 1
IRQ 8,  40  ; CMOS Real Time Clock
IRQ 9,  41  ; CGA Vertical Retrace
IRQ 10, 42  ; Reserved
IRQ 11, 43  ; Reserved
IRQ 12, 44  ; AT systems: Reserved. PS/2: Auxiliary Device
IRQ 13, 45  ; FPU
IRQ 14, 36  ; Hard Disk Controller
IRQ 15, 47  ; Reserved