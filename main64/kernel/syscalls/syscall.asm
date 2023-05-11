[BITS 64]
[GLOBAL SysCallHandlerAsm]
[EXTERN SysCallHandlerC]

; Virtual address where the SysCallRegisters structure will be stored
SYSCALLREGISTERS_OFFSET    EQU 0xFFFF800000064000

SysCallHandlerAsm:
    CLI

    ; Save the General Purpose registers on the Stack
    PUSH    RBX
    PUSH    RAX
    PUSH    RCX
    PUSH    RDX
    PUSH    RSI
    PUSH    RDI
    PUSH    RBP
    PUSH    RSP
    PUSH    R8
    PUSH    R9
    PUSH    R10
    PUSH    R11
    PUSH    R12
    PUSH    R13
    PUSH    R14
    PUSH    R15

    ; Store the RDI Number
    MOV     RAX, SYSCALLREGISTERS_OFFSET
    MOV     [RAX], RDI

    ; Store the RSI register
    ADD     RAX, 0x8
    MOV     [RAX], RSI

    ; Store the RDX register
    ADD     RAX, 0x8
    MOV     [RAX], RDX

    ; Store the RCX register
    ADD     RAX, 0x8
    MOV     [RAX], RCX

    ; Store the R8 register
    ADD     RAX, 0x8
    MOV     [RAX], R8

    ; Store the R9 register
    ADD     RAX, 0x8
    MOV     [RAX], R9

    ; Call the ISR handler that is implemented in C
    MOV     RDI, SYSCALLREGISTERS_OFFSET 
    CALL    SysCallHandlerC

    ; Restore the General Purpose registers from the Stack
    POP     R15
    POP     R14
    POP     R13
    POP     R12
    POP     R11
    POP     R10
    POP     R9
    POP     R8
    POP     RSP
    POP     RBP
    POP     RDI
    POP     RSI
    POP     RDX
    POP     RCX
    
    ; We don't restore the RAX register, because it contains the result of the SysCall (from the function call to "SysCallHandlerC").
    ; So we just pop the old RAX value into RBX and then pop the old RBX value into RBX
    POP     RBX     ; Original RAX value
    POP     RBX

    STI
    IRETQ