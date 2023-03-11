[BITS 64]
[GLOBAL SysCallHandlerAsm]
[GLOBAL RaiseSysCallAsm]
[EXTERN SysCallHandlerC]

SysCallHandlerAsm:
    CLI

    ; Save the General Purpose registers on the Stack
    PUSH    RDI
    PUSH    RSI
    PUSH    RBP
    PUSH    RSP
    PUSH    RBX
    PUSH    RDX
    PUSH    RCX
    PUSH    RAX
    PUSH    R8
    PUSH    R9
    PUSH    R10
    PUSH    R11
    PUSH    R12
    PUSH    R13
    PUSH    R14
    PUSH    R15

    ; Call the ISR handler that is implemented in C
    MOV     RDI, RAX
    MOV     RSI, RBX
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
    
    ; We don't restore the RAX register, because it contains the result of the SysCall (from the function call to "SysCallHandlerC").
    ; So we just pop the old RAX value into RCX and then pop the old RCX value into RCX
    POP     RCX
    POP     RCX
    
    ; Restore the remaining registers from the Stack
    POP     RDX
    POP     RBX
    POP     RSP
    POP     RBP
    POP     RSI
    POP     RDI

    STI
    IRETQ

; Raises a SysCall
RaiseSysCallAsm:
    MOV     RAX, RDI    ; SysCall Number
    MOV     RBX, RSI    ; SysCall Parameter
    INT     0x80
    RET