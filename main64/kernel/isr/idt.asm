[BITS 64]
[EXTERN IsrHandler]

; Needed, so that the C code can call the assembly functions
[GLOBAL IdtFlush]
[GLOBAL DisableInterrupts]
[GLOBAL EnableInterrupts]

; Virtual address where the RegisterState structure will be stored
REGISTERSTATE_OFFSET    EQU 0xFFFF800000061000

; Loads the IDT table
IdtFlush:
    ; The first function parameter is provided in the RDI register on the x64 architecture
    ; RDI points to the variable idtPointer (defined in the C code)
    LIDT    [RDI]
    RET

; Disables the hardware interrupts
DisableInterrupts:
    CLI
    RET

; Enables the hardware interrupts
EnableInterrupts:
    STI
    RET

; The following macro emits the ISR assembly routine
%MACRO ISR_NOERRORCODE 1
    [GLOBAL Isr%1]
    Isr%1:
        CLI

        ; Produce a new Stack Frame
        PUSH    RBP     ; [RSP + 128]
        MOV     RBP, RSP

        ; Save the *original* general purpose register values on the Stack, when the interrupt has occured.
        ; These *original* values will be passed through the structure "RegisterState" to the C function "IsrHandler".
        PUSH    RDI    ; [RSP + 120]
        PUSH    RSI    ; [RSP + 112]
        PUSH    RBP    ; [RSP + 104]
        PUSH    RSP    ; [RSP + 96]
        PUSH    RAX    ; [RSP + 88]
        PUSH    RBX    ; [RSP + 80]
        PUSH    RCX    ; [RSP + 72]
        PUSH    RDX    ; [RSP + 64]
        PUSH    R8     ; [RSP + 56]
        PUSH    R9     ; [RSP + 48]
        PUSH    R10    ; [RSP + 40]
        PUSH    R11    ; [RSP + 32]
        PUSH    R12    ; [RSP + 24]
        PUSH    R13    ; [RSP + 16]
        PUSH    R14    ; [RSP + 8]
        PUSH    R15    ; [RSP + 0]

        ; Now we have the following stack layout:
        ; [RSP + 136]: RIP where the Exception has occured
        ; [RSP + 128]: Original RBP register value when we produced the new Stack Frame
        ; [RSP + 120]: Original RDI register value
        ; [RSP + 112]: Original RSI register value 
        ; [RSP + 104]: Original RBP register value
        ; [RSP +  96]: Original RSP register value
        ; [RSP +  88]: Original RAX register value
        ; [RSP +  80]: Original RBX register value
        ; [RSP +  72]: Original RCX register value
        ; [RSP +  64]: Original RDX register value
        ; [RSP +  56]: Original R8 register value
        ; [RSP +  48]: Original R9 register value
        ; [RSP +  40]: Original R10 register value
        ; [RSP +  32]: Original R11 register value
        ; [RSP +  24]: Original R12 register value
        ; [RSP +  16]: Original R13 register value
        ; [RSP +   8]: Original R14 register value
        ; [RSP +   0]: Original R15 register value
        
        ; Store the RIP
        MOV     RAX, REGISTERSTATE_OFFSET
        MOV     RBX, [RSP + 136]
        MOV     [RAX], RBX

        ; Store the Error Code
        ADD     RAX, 0x8
        MOV     RBX, 0x0    ; There is no Error Code
        MOV     [RAX], RBX

        ; Store the RDI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 120]
        MOV     [RAX], RBX

        ; Store the RSI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 112]
        MOV     [RAX], RBX

        ; Store the RBP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 104]
        MOV     [RAX], RBX

        ; Store the RSP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 96]
        MOV     [RAX], RBX

        ; Store the RAX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 88]
        MOV     [RAX], RBX

        ; Store the RBX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 80]
        MOV     [RAX], RBX

        ; Store the RCX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 72]
        MOV     [RAX], RBX

        ; Store the RDX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 64]
        MOV     [RAX], RBX

        ; Store the R8 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 56]
        MOV     [RAX], RBX

        ; Store the R9 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 48]
        MOV     [RAX], RBX

        ; Store the R10 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 40]
        MOV     [RAX], RBX

        ; Store the R11 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 32]
        MOV     [RAX], RBX

        ; Store the R12 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 24]
        MOV     [RAX], RBX

        ; Store the R13 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 16]
        MOV     [RAX], RBX

        ; Store the R14 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 8]
        MOV     [RAX], RBX

        ; Store the R15 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 0]
        MOV     [RAX], RBX

        ; Call the ISR handler that is implemented in C
        MOV     RDI, %1                     ; 1st parameter
        MOV     RSI, CR2                    ; 2nd parameter
        MOV     RDX, REGISTERSTATE_OFFSET   ; Set the 3rd parameter to the memory location where the structure with the RegisterState is stored
        CALL    IsrHandler

        ; Restore the *original* general purpose register values from the Stack
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

        ; Restore the Stack Base Pointer
        POP     RBP

        STI
        IRETQ
%ENDMACRO

; The following macro emits the ISR assembly routine
%MACRO ISR_ERRORCODE 1
    [GLOBAL Isr%1]
    Isr%1:
        CLI

        ; Produce a new Stack Frame
        PUSH    RBP    ; [RSP + 128]
        MOV     RBP, RSP
        
        ; Save the *original* general purpose register values on the Stack, when the interrupt has occured.
        ; These *original* values will be passed through the structure "RegisterState" to the C function "IsrHandler".
        PUSH    RDI    ; [RSP + 120]
        PUSH    RSI    ; [RSP + 112]
        PUSH    RBP    ; [RSP + 104]
        PUSH    RSP    ; [RSP + 96]
        PUSH    RAX    ; [RSP + 88]
        PUSH    RBX    ; [RSP + 80]
        PUSH    RCX    ; [RSP + 72]
        PUSH    RDX    ; [RSP + 64]
        PUSH    R8     ; [RSP + 56]
        PUSH    R9     ; [RSP + 48]
        PUSH    R10    ; [RSP + 40]
        PUSH    R11    ; [RSP + 32]
        PUSH    R12    ; [RSP + 24]
        PUSH    R13    ; [RSP + 16]
        PUSH    R14    ; [RSP + 8]
        PUSH    R15    ; [RSP + 0]

        ; Now we have the following stack layout:
        ; [RSP + 144]: RIP where the Exception has occured
        ; [RSP + 136]: Error Code
        ; [RSP + 128]: Original RBP register value when we produced the new Stack Frame
        ; [RSP + 120]: Original RDI register value
        ; [RSP + 112]: Original RSI register value 
        ; [RSP + 104]: Original RBP register value
        ; [RSP +  96]: Original RSP register value
        ; [RSP +  88]: Original RAX register value
        ; [RSP +  80]: Original RBX register value
        ; [RSP +  72]: Original RCX register value
        ; [RSP +  64]: Original RDX register value
        ; [RSP +  56]: Original R8 register value
        ; [RSP +  48]: Original R9 register value
        ; [RSP +  40]: Original R10 register value
        ; [RSP +  32]: Original R11 register value
        ; [RSP +  24]: Original R12 register value
        ; [RSP +  16]: Original R13 register value
        ; [RSP +   8]: Original R14 register value
        ; [RSP +   0]: Original R15 register value
        
        ; Store the RIP
        MOV     RAX, REGISTERSTATE_OFFSET
        MOV     RBX, [RSP + 144]
        MOV     [RAX], RBX

        ; Store the Error Code
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 136]
        MOV     [RAX], RBX

        ; Store the RDI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 120]
        MOV     [RAX], RBX

        ; Store the RSI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 112]
        MOV     [RAX], RBX

        ; Store the RBP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 104]
        MOV     [RAX], RBX

        ; Store the RSP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 96]
        MOV     [RAX], RBX

        ; Store the RAX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 88]
        MOV     [RAX], RBX

        ; Store the RBX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 80]
        MOV     [RAX], RBX

        ; Store the RCX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 72]
        MOV     [RAX], RBX

        ; Store the RDX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 64]
        MOV     [RAX], RBX

        ; Store the R8 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 56]
        MOV     [RAX], RBX

        ; Store the R9 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 48]
        MOV     [RAX], RBX

        ; Store the R10 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 40]
        MOV     [RAX], RBX

        ; Store the R11 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 32]
        MOV     [RAX], RBX

        ; Store the R12 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 24]
        MOV     [RAX], RBX

        ; Store the R13 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 16]
        MOV     [RAX], RBX

        ; Store the R14 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 8]
        MOV     [RAX], RBX

        ; Store the R15 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 0]
        MOV     [RAX], RBX

        ; Call the ISR handler that is implemented in C
        MOV     RDI, %1                     ; 1st parameter
        MOV     RSI, CR2                    ; 2nd parameter
        MOV     RDX, REGISTERSTATE_OFFSET   ; Set the 3rd parameter to the memory location where the structure with the RegisterState is stored
        CALL    IsrHandler

        ; Restore the *original* general purpose register values from the Stack
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

        ; Restore the Stack Base Pointer
        POP     RBP

        ; Remove the Error Code from the Stack
        ADD     RSP, 8

        ; Return from the ISR routine...
        STI
        IRETQ
%ENDMACRO

; Emitting our 32 ISR assembly routines
ISR_NOERRORCODE 0   ; Divide Error Exception
ISR_NOERRORCODE 1   ; Debug Exception
ISR_NOERRORCODE 2   ; Non-Maskable Interrupt
ISR_NOERRORCODE 3   ; Breakpoint Exception
ISR_NOERRORCODE 4   ; Overflow Exception
ISR_NOERRORCODE 5   ; Bound Range Exceeded Exception
ISR_NOERRORCODE 6   ; Invalid Opcode Exception
ISR_NOERRORCODE 7   ; Device Not Available Exception
ISR_ERRORCODE   8   ; Double Fault Exception
ISR_NOERRORCODE 9   ; Coprocessor Segment Overrun
ISR_ERRORCODE   10  ; Invalid TSS Exception
ISR_ERRORCODE   11  ; Segment Not Present
ISR_ERRORCODE   12  ; Stack Fault Exception
ISR_ERRORCODE   13  ; General Protection Exception
ISR_ERRORCODE   14  ; Page Fault Exception
ISR_NOERRORCODE 15  ; Unassigned!
ISR_NOERRORCODE 16  ; x87 FPU Floating Point Error
ISR_NOERRORCODE 17  ; Alignment Check Exception
ISR_NOERRORCODE 18  ; Machine Check Exception
ISR_NOERRORCODE 19  ; SIMD Floating Point Exception
ISR_NOERRORCODE 20  ; Virtualization Exception
ISR_NOERRORCODE 21  ; Control Protection Exception
ISR_NOERRORCODE 22  ; Reserved!
ISR_NOERRORCODE 23  ; Reserved!
ISR_NOERRORCODE 24  ; Reserved!
ISR_NOERRORCODE 25  ; Reserved!
ISR_NOERRORCODE 26  ; Reserved!
ISR_NOERRORCODE 27  ; Reserved!
ISR_NOERRORCODE 28  ; Reserved!
ISR_NOERRORCODE 29  ; Reserved!
ISR_NOERRORCODE 30  ; Reserved!
ISR_NOERRORCODE 31  ; Reserved!