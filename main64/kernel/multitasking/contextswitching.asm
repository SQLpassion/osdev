[BITS 64]
[GLOBAL Irq0_ContextSwitching]
[GLOBAL GetTaskState]
[EXTERN MoveToNextTask]

; =======================================================================
; The following constants defines the offsets into the C structure "Task"
; =======================================================================

; Instruction Pointer and Flags Registers
%DEFINE TaskState_RIP       0
%DEFINE TaskState_RFLAGS    8

; General Purpose Registers
%DEFINE TaskState_RAX       16
%DEFINE TaskState_RBX       24
%DEFINE TaskState_RCX       32
%DEFINE TaskState_RDX       40 
%DEFINE TaskState_RSI       48
%DEFINE TaskState_RDI       56
%DEFINE TaskState_RBP       64
%DEFINE TaskState_RSP       72
%DEFINE TaskState_R8        80
%DEFINE TaskState_R9        88
%DEFINE TaskState_R10       96
%DEFINE TaskState_R11       104
%DEFINE TaskState_R12       112
%DEFINE TaskState_R13       120
%DEFINE TaskState_R14       128
%DEFINE TaskState_R15       136

; Segment Registers
%DEFINE TaskState_SS        144
%DEFINE TaskState_CS        152
%DEFINE TaskState_DS        160
%DEFINE TaskState_ES        168
%DEFINE TaskState_FS        176
%DEFINE TaskState_GS        184

; Control Registers
%DEFINE TaskState_CR3       192

; ============================================================================
; The following constants defines the offsets into the IRQ Stack Frame Layout
; IRQ STACK FRAME LAYOUT (based on the current RSP):
; ============================================================================
; Return SS:        +32
; Return RSP:       +24
; Return RFLAGS:    +16
; Return CS:        +8
; Return RIP:       +0
; 
%DEFINE StackFrame_RIP      0
%DEFINE StackFrame_CS       8
%DEFINE StackFrame_RFLAGS   16
%DEFINE StackFrame_RSP      24
%DEFINE StackFrame_SS       32

; This function is called as soon as the Timer Interrupt is raised
; 
; NOTE: We don't need to disable/enable the interrupts explicitly, because the IRQ0 is an Interrupt Gate,
; where the interrupts are disabled/enabled automatically by the CPU!
Irq0_ContextSwitching:
    CLI

    ; Save RDI on the Stack, so that we can store it later in the Task structure
    PUSH    RDI

    ; The first initial code execution path (entry point of KERNEL.BIN) that was started by KLDR64.BIN,
    ; has no Task structure assigned in register R15.
    ; Therefore we only save the current Task State if we have a Task structure assigned in R15.
    MOV     RDI, R15
    CMP     RDI, 0x0
    JE      NoTaskStateSaveNecessary
    
    ; Save the current general purpose registers
    MOV     [RDI + TaskState_RAX], RAX
    MOV     [RDI + TaskState_RBX], RBX
    MOV     [RDI + TaskState_RCX], RCX
    MOV     [RDI + TaskState_RDX], RDX
    MOV     [RDI + TaskState_RSI], RSI
    MOV     [RDI + TaskState_RBP], RBP
    MOV     [RDI + TaskState_R8],  R8
    MOV     [RDI + TaskState_R9],  R9
    MOV     [RDI + TaskState_R10], R10
    MOV     [RDI + TaskState_R11], R11
    MOV     [RDI + TaskState_R12], R12
    MOV     [RDI + TaskState_R13], R13
    MOV     [RDI + TaskState_R14], R14
    MOV     [RDI + TaskState_R15], R15

    ; Save RDI
    POP     RAX ; Pop the initial content of RDI off the Stack
    MOV     [RDI + TaskState_RDI], RAX

    ; Save the Segment Registers
    MOV     [RDI + TaskState_DS], DS
    MOV     [RDI + TaskState_ES], ES
    MOV     [RDI + TaskState_FS], FS
    MOV     [RDI + TaskState_GS], GS

    ; IRQ STACK FRAME LAYOUT (based on the current RSP)
    ; ==================================================
    ; Return SS:        +32
    ; Return RSP:       +24
    ; Return RFLAGS:    +16
    ; Return CS:        +8
    ; Return RIP:       +0

    ; Save the current register RIP from the Stack
    MOV     RAX, [RSP + StackFrame_RIP]
    MOV     [RDI + TaskState_RIP], RAX

    ; Save the current register CS from the Stack
    MOV     RAX, [RSP + StackFrame_CS]
    MOV     [RDI + TaskState_CS], RAX

    ; Save the current register RFLAGS from the Stack
    MOV     RAX, [RSP + StackFrame_RFLAGS]
    MOV     [RDI + TaskState_RFLAGS], RAX

    ; Save the current register RSP from the Stack
    MOV     RAX, [RSP + StackFrame_RSP]
    MOV     [RDI + TaskState_RSP], RAX

    ; Save the current register SS from the Stack
    MOV     RAX, [RSP + StackFrame_SS]
    MOV     [RDI + TaskState_SS], RAX

    JMP     Continue

NoTaskStateSaveNecessary:
    ; Pop the initial content of RDI off the Stack
    POP     RAX

Continue:
    ; Move to the next Task to be executed
    CALL    MoveToNextTask

    ; Store the pointer to the current Task in the register RDI.
    ; It was returned in the register RAX from the previous function call.
    MOV     RDI, RAX
    
    ; Restore the general purpose registers of the next Task to be executed
    MOV     RBX, [RDI + TaskState_RBX]
    MOV     RCX, [RDI + TaskState_RCX]
    MOV     RDX, [RDI + TaskState_RDX]
    MOV     RSI, [RDI + TaskState_RSI]
    MOV     RBP, [RDI + TaskState_RBP]
    MOV     R8,  [RDI + TaskState_R8]
    MOV     R9,  [RDI + TaskState_R9]
    MOV     R10, [RDI + TaskState_R10]
    MOV     R11, [RDI + TaskState_R11]
    MOV     R12, [RDI + TaskState_R12]
    MOV     R13, [RDI + TaskState_R13]
    MOV     R14, [RDI + TaskState_R14]
    MOV     R15, [RDI + TaskState_R15]

    ; IRQ STACK FRAME LAYOUT (based on the current RSP)
    ; ==================================================
    ; Return SS:        +32
    ; Return RSP:       +24
    ; Return RFLAGS:    +16
    ; Return CS:        +8
    ; Return RIP:       +0

    ; Restore the register RIP of the next Task onto the stack
    MOV     RAX, [RDI + TaskState_RIP]
    MOV     [RSP + StackFrame_RIP], RAX

    ; Restore the register CS of the next Task onto the stack
    MOV     RAX, [RDI + TaskState_CS]
    MOV     [RSP + StackFrame_CS], RAX
    
    ; Restore the register RFLAGS of the next Task onto the stack
    MOV     RAX, [RDI + TaskState_RFLAGS]
    MOV     [RSP + StackFrame_RFLAGS], RAX

    ; Restore the register RSP of the next Task onto the stack
    MOV     RAX, [RDI + TaskState_RSP]
    MOV     [RSP + StackFrame_RSP], RAX

    ; Restore the register SS of the next Task onto the stack
    MOV     RAX, [RDI + TaskState_SS]
    MOV     [RSP + StackFrame_SS], RAX
    
    ; Restore the register RAX register of the next Task
    MOV     RAX, [RDI + TaskState_RAX]

    ; Restore the remaining Segment Registers
    MOV     DS, [RDI + TaskState_DS]
    MOV     ES, [RDI + TaskState_ES]
    MOV     FS, [RDI + TaskState_FS]
    MOV     GS, [RDI + TaskState_GS]

    ; Send the reset signal to the master PIC...
    PUSH    RAX
    MOV     RAX, 0x20
    OUT     0x20, EAX
    POP     RAX

    ; Return from the Interrupt Handler
    ; Because we have patched the Stack Frame of the Interrupt Handler, we continue with the execution of 
    ; the next Task - based on the restored register RIP on the Stack...
    STI
    IRETQ

; This function returns a pointer to the Task structure of the current executing Task
GetTaskState:
    MOV     RAX, R15
    RET