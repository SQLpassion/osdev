[BITS 64]
[EXTERN IsrHandler]

; Needed, so that the C code can call the assembly functions
[GLOBAL IdtFlush]
[GLOBAL DisableInterrupts]
[GLOBAL EnableInterrupts]

; =======================================================================
; The following constants defines the offsets for the various registers
; which are pushed onto the stack.
; =======================================================================
%DEFINE StackOffset_RAX     176
%DEFINE StackOffset_RBX     168
%DEFINE StackOffset_RCX     160
%DEFINE StackOffset_RDX     152
%DEFINE StackOffset_RSI     144
%DEFINE StackOffset_RDI     136
%DEFINE StackOffset_RBP     128
%DEFINE StackOffset_RSP     120
%DEFINE StackOffset_R8      112
%DEFINE StackOffset_R9      104
%DEFINE StackOffset_R10     96
%DEFINE StackOffset_R11     88
%DEFINE StackOffset_R12     80
%DEFINE StackOffset_R13     72
%DEFINE StackOffset_R14     64
%DEFINE StackOffset_R15     56
%DEFINE StackOffset_SS      48
%DEFINE StackOffset_CS      40
%DEFINE StackOffset_DS      32
%DEFINE StackOffset_ES      24
%DEFINE StackOffset_FS      16
%DEFINE StackOffset_GS      8
%DEFINE StackOffset_CR3     0

; Virtual address where the RegisterState structure will be stored
REGISTERSTATE_OFFSET    EQU 0xFFFF800000063000

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
        PUSH    RAX         ; [RSP + 176]
        PUSH    RBX         ; [RSP + 168]
        PUSH    RCX         ; [RSP + 160]
        PUSH    RDX         ; [RSP + 152]
        PUSH    RSI         ; [RSP + 144]
        PUSH    RDI         ; [RSP + 136]
        PUSH    RBP         ; [RSP + 128]
        PUSH    RSP         ; [RSP + 120]
        PUSH    R8          ; [RSP + 112]
        PUSH    R9          ; [RSP + 104]
        PUSH    R10         ; [RSP +  96]
        PUSH    R11         ; [RSP +  88]
        PUSH    R12         ; [RSP +  80]
        PUSH    R13         ; [RSP +  72]
        PUSH    R14         ; [RSP +  64]
        PUSH    R15         ; [RSP +  56]
        MOV     RAX, SS   
        PUSH    RAX         ; [RSP +  48]
        MOV     RAX, CS   
        PUSH    RAX         ; [RSP +  40]
        MOV     RAX, DS   
        PUSH    RAX         ; [RSP +  32]
        MOV     RAX, ES   
        PUSH    RAX         ; [RSP +  24]
        MOV     RAX, FS   
        PUSH    RAX         ; [RSP +  16]
        MOV     RAX, GS   
        PUSH    RAX         ; [RSP +   8]
        MOV     RAX, CR3    
        PUSH    RAX         ; [RSP +   0]

        ; Now we have the following stack layout:
        ; [RSP + 192]: RIP where the Exception has occured
        ; [RSP + 184]: Original RBP register value when we produced the new Stack Frame
        ; [RSP + 176]: Original RAX register value
        ; [RSP + 168]: Original RBX register value
        ; [RSP + 160]: Original RCX register value
        ; [RSP + 152]: Original RDX register value
        ; [RSP + 144]: Original RSI register value
        ; [RSP + 136]: Original RDI register value 
        ; [RSP + 128]: Original RBP register value
        ; [RSP + 120]: Original RSP register value
        ; [RSP + 112]: Original R8 register value
        ; [RSP + 104]: Original R9 register value
        ; [RSP +  96]: Original R10 register value
        ; [RSP +  88]: Original R11 register value
        ; [RSP +  80]: Original R12 register value
        ; [RSP +  72]: Original R13 register value
        ; [RSP +  64]: Original R14 register value
        ; [RSP +  56]: Original R15 register value
        ; [RSP +  48]: Original SS register value
        ; [RSP +  40]: Original CS register value
        ; [RSP +  32]: Original DS register value
        ; [RSP +  24]: Original ES register value
        ; [RSP +  16]: Original FS register value
        ; [RSP +   8]: Original GS register value
        ; [RSP +   0]: Original CR3 register value
        
        ; Store the RIP
        MOV     RAX, REGISTERSTATE_OFFSET
        MOV     RBX, [RSP + 192]
        MOV     [RAX], RBX

        ; Store the Error Code
        ADD     RAX, 0x8
        MOV     RBX, 0x0    ; There is no Error Code
        MOV     [RAX], RBX

        ; Store the RAX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RAX]
        MOV     [RAX], RBX

        ; Store the RBX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RBX]
        MOV     [RAX], RBX

        ; Store the RCX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RCX]
        MOV     [RAX], RBX

        ; Store the RDX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RDX]
        MOV     [RAX], RBX

        ; Store the RSI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RSI]
        MOV     [RAX], RBX

        ; Store the RDI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RDI]
        MOV     [RAX], RBX

        ; Store the RBP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RBP]
        MOV     [RAX], RBX

        ; Store the RSP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RSP]
        MOV     [RAX], RBX

        ; Store the R8 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R8]
        MOV     [RAX], RBX

        ; Store the R9 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R9]
        MOV     [RAX], RBX

        ; Store the R10 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R10]
        MOV     [RAX], RBX

        ; Store the R11 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R11]
        MOV     [RAX], RBX

        ; Store the R12 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R12]
        MOV     [RAX], RBX

        ; Store the R13 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R13]
        MOV     [RAX], RBX

        ; Store the R14 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R14]
        MOV     [RAX], RBX

        ; Store the R15 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R15]
        MOV     [RAX], RBX

        ; Store the SS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_SS]
        MOV     [RAX], RBX

        ; Store the CS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_CS]
        MOV     [RAX], RBX

        ; Store the DS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_DS]
        MOV     [RAX], RBX

        ; Store the ES register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_ES]
        MOV     [RAX], RBX

        ; Store the FS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_FS]
        MOV     [RAX], RBX

        ; Store the GS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_GS]
        MOV     [RAX], RBX

        ; Store the CR3 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_CR3]
        MOV     [RAX], RBX

        ; Call the ISR handler that is implemented in C
        MOV     RDI, %1                     ; 1st parameter
        MOV     RSI, CR2                    ; 2nd parameter
        MOV     RDX, REGISTERSTATE_OFFSET   ; Set the 3rd parameter to the memory location where the structure with the RegisterState is stored
        CALL    IsrHandler

        ; Restore the *original* general purpose register values from the Stack
        POP     RAX     ; CR3
        POP     RAX     ; SS
        POP     RAX     ; CS
        POP     RAX     ; DS
        POP     RAX     ; ES
        POP     RAX     ; FS
        POP     RAX     ; GS
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
        POP     RBX
        POP     RAX

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
        PUSH    RBP         ; [RSP + 128]
        MOV     RBP, RSP
        
        ; Save the *original* general purpose register values on the Stack, when the interrupt has occured.
        ; These *original* values will be passed through the structure "RegisterState" to the C function "IsrHandler".
        PUSH    RAX         ; [RSP + 176]
        PUSH    RBX         ; [RSP + 168]
        PUSH    RCX         ; [RSP + 160]
        PUSH    RDX         ; [RSP + 152]
        PUSH    RSI         ; [RSP + 144]
        PUSH    RDI         ; [RSP + 136]
        PUSH    RBP         ; [RSP + 128]
        PUSH    RSP         ; [RSP + 120]
        PUSH    R8          ; [RSP + 112]
        PUSH    R9          ; [RSP + 104]
        PUSH    R10         ; [RSP +  96]
        PUSH    R11         ; [RSP +  88]
        PUSH    R12         ; [RSP +  80]
        PUSH    R13         ; [RSP +  72]
        PUSH    R14         ; [RSP +  64]
        PUSH    R15         ; [RSP +  56]
        MOV     RAX, SS   
        PUSH    RAX         ; [RSP +  48]
        MOV     RAX, CS   
        PUSH    RAX         ; [RSP +  40]
        MOV     RAX, DS   
        PUSH    RAX         ; [RSP +  32]
        MOV     RAX, ES   
        PUSH    RAX         ; [RSP +  24]
        MOV     RAX, FS   
        PUSH    RAX         ; [RSP +  16]
        MOV     RAX, GS   
        PUSH    RAX         ; [RSP +   8]
        MOV     RAX, CR3    
        PUSH    RAX         ; [RSP +   0]

        ; Now we have the following stack layout:
        ; [RSP + 200]: RIP where the Exception has occured
        ; [RSP + 192]: Error Code
        ; [RSP + 184]: Original RBP register value when we produced the new Stack Frame
        ; [RSP + 176]: Original RAX register value
        ; [RSP + 168]: Original RBX register value
        ; [RSP + 160]: Original RCX register value
        ; [RSP + 152]: Original RDX register value
        ; [RSP + 144]: Original RSI register value
        ; [RSP + 136]: Original RDI register value 
        ; [RSP + 128]: Original RBP register value
        ; [RSP + 120]: Original RSP register value
        ; [RSP + 112]: Original R8 register value
        ; [RSP + 104]: Original R9 register value
        ; [RSP +  96]: Original R10 register value
        ; [RSP +  88]: Original R11 register value
        ; [RSP +  80]: Original R12 register value
        ; [RSP +  72]: Original R13 register value
        ; [RSP +  64]: Original R14 register value
        ; [RSP +  56]: Original R15 register value
        ; [RSP +  48]: Original SS register value
        ; [RSP +  40]: Original CS register value
        ; [RSP +  32]: Original DS register value
        ; [RSP +  24]: Original ES register value
        ; [RSP +  16]: Original FS register value
        ; [RSP +   8]: Original GS register value
        ; [RSP +   0]: Original CR3 register value
        
        ; Store the RIP
        MOV     RAX, REGISTERSTATE_OFFSET
        MOV     RBX, [RSP + 200]
        MOV     [RAX], RBX

        ; Store the Error Code
        ADD     RAX, 0x8
        MOV     RBX, [RSP + 192]
        MOV     [RAX], RBX

        ; Store the RAX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RAX]
        MOV     [RAX], RBX

        ; Store the RBX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RBX]
        MOV     [RAX], RBX

        ; Store the RCX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RCX]
        MOV     [RAX], RBX

        ; Store the RDX register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RDX]
        MOV     [RAX], RBX

        ; Store the RSI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RSI]
        MOV     [RAX], RBX

        ; Store the RDI register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RDI]
        MOV     [RAX], RBX

        ; Store the RBP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RBP]
        MOV     [RAX], RBX

        ; Store the RSP register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_RSP]
        MOV     [RAX], RBX

        ; Store the R8 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R8]
        MOV     [RAX], RBX

        ; Store the R9 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R9]
        MOV     [RAX], RBX

        ; Store the R10 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R10]
        MOV     [RAX], RBX

        ; Store the R11 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R11]
        MOV     [RAX], RBX

        ; Store the R12 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R12]
        MOV     [RAX], RBX

        ; Store the R13 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R13]
        MOV     [RAX], RBX

        ; Store the R14 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R14]
        MOV     [RAX], RBX

        ; Store the R15 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_R15]
        MOV     [RAX], RBX

        ; Store the SS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_SS]
        MOV     [RAX], RBX

        ; Store the CS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_CS]
        MOV     [RAX], RBX

        ; Store the DS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_DS]
        MOV     [RAX], RBX

        ; Store the ES register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_ES]
        MOV     [RAX], RBX

        ; Store the FS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_FS]
        MOV     [RAX], RBX

        ; Store the GS register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_GS]
        MOV     [RAX], RBX

        ; Store the CR3 register
        ADD     RAX, 0x8
        MOV     RBX, [RSP + StackOffset_CR3]
        MOV     [RAX], RBX

        ; Call the ISR handler that is implemented in C
        MOV     RDI, %1                     ; 1st parameter
        MOV     RSI, CR2                    ; 2nd parameter
        MOV     RDX, REGISTERSTATE_OFFSET   ; Set the 3rd parameter to the memory location where the structure with the RegisterState is stored
        CALL    IsrHandler

        ; Restore the *original* general purpose register values from the Stack
        POP     RAX     ; CR3
        POP     RAX     ; SS
        POP     RAX     ; CS
        POP     RAX     ; DS
        POP     RAX     ; ES
        POP     RAX     ; FS
        POP     RAX     ; GS
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
        POP     RBX
        POP     RAX

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