[BITS 64]
[GLOBAL Irq0_ContextSwitching]
[GLOBAL GetTaskState]
[EXTERN MoveToNextTask]

; Defines the various offsets into the C structure "Task"
%define TaskState_RAX       0
%define TaskState_RBX       8
%define TaskState_RCX       16
%define TaskState_RDX       24 
%define TaskState_RBP       32
%define TaskState_RSI       40
%define TaskState_R8        48
%define TaskState_R9        56
%define TaskState_R10       64
%define TaskState_R11       72
%define TaskState_R12       80
%define TaskState_R13       88
%define TaskState_R14       96
%define TaskState_R15       104
%define TaskState_CR3       112
%define TaskState_RDI       120
%define TaskState_RIP       128
%define TaskState_CS        136
%define TaskState_RFLAGS    144
%define TaskState_RSP       152
%define TaskState_SS        160
%define TaskState_DS        168
%define TaskState_ES        176
%define TaskState_FS        184
%define TaskState_GS        192

; This function is called as soon as the Timer Interrupt is raised
; 
; NOTE: We don't need to disable/enable the interrupts explicitly, because the IRQ0 is an Interrupt Gate,
; where the interrupts are disabled/enabled automatically by the CPU!
Irq0_ContextSwitching:
    ; Save RDI on the Stack, so that we can store it later in the Task structure
    push rdi

    ; The first initial code execution path (entry point of KERNEL.BIN) that was started by KLDR64.BIN,
    ; has no Task structure assigned in register R15.
    ; Therefore we only save the current Task State if we have a Task structure assigned in R15.
    mov rdi, r15
    cmp rdi, 0x0
    je NoTaskStateSaveNecessary
    
    ; Save the current general purpose registers
    mov [rdi + TaskState_RAX], rax
    mov [rdi + TaskState_RBX], rbx
    mov [rdi + TaskState_RCX], rcx
    mov [rdi + TaskState_RDX], rdx
    mov [rdi + TaskState_RBP], rbp
    mov [rdi + TaskState_RSI], rsi
    mov [rdi + TaskState_R8],  r8
    mov [rdi + TaskState_R9],  r9
    mov [rdi + TaskState_R10], r10
    mov [rdi + TaskState_R11], r11
    mov [rdi + TaskState_R12], r12
    mov [rdi + TaskState_R13], r13
    mov [rdi + TaskState_R14], r14
    mov [rdi + TaskState_R15], r15

    ; Save RDI
    pop rax ; Pop the initial content of RDI off the Stack
    mov [rdi + TaskState_RDI], rax

    ; Save the Segment Registers
    mov [rdi + TaskState_DS], ds
    mov [rdi + TaskState_ES], es
    mov [rdi + TaskState_FS], fs
    mov [rdi + TaskState_GS], gs

    ; IRQ STACK FRAME LAYOUT (based on the current RSP)
    ; ==================================================
    ; Return SS:        + 32
    ; Return RSP:       + 24
    ; Return RFLAGS:    + 16
    ; Return CS:        + 8
    ; Return RIP:       + 0

    ; Save the current register RIP from the Stack
    mov rax, [rsp + 0]
    mov [rdi + TaskState_RIP], rax

    ; Save the current register CS from the Stack
    mov rax, [rsp + 8]
    mov [rdi + TaskState_CS], rax

    ; Save the current register RFLAGS from the Stack
    mov rax, [rsp + 16]
    mov [rdi + TaskState_RFLAGS], rax

    ; Save the current register RSP from the Stack
    mov rax, [rsp + 24]
    mov [rdi + TaskState_RSP], rax

    ; Save the current register SS from the Stack
    mov rax, [rsp + 32]
    mov [rdi + TaskState_SS], rax

    jmp Continue

NoTaskStateSaveNecessary:
    ; Pop the initial content of RDI off the Stack
    pop rax

Continue:
    ; Move to the next Task to be executed
    call MoveToNextTask

    ; Store the pointer to the current Task in the register RDI.
    ; It was returned in the register RAX from the previous function call.
    mov rdi, rax
    
    ; Restore the general purpose registers of the next Task to be executed
    mov rbx, [rdi + TaskState_RBX]
    mov rcx, [rdi + TaskState_RCX]
    mov rdx, [rdi + TaskState_RDX]
    mov rbp, [rdi + TaskState_RBP]
    mov rsi, [rdi + TaskState_RSI]
    mov r8, [rdi + TaskState_R8]
    mov r9, [rdi + TaskState_R9]
    mov r10, [rdi + TaskState_R10]
    mov r11, [rdi + TaskState_R11]
    mov r12, [rdi + TaskState_R12]
    mov r13, [rdi + TaskState_R13]
    mov r14, [rdi + TaskState_R14]
    mov r15, [rdi + TaskState_R15]

    ; IRQ STACK FRAME LAYOUT (based on the current RSP)
    ; ==================================================
    ; Return SS:        + 32
    ; Return RSP:       + 24
    ; Return RFLAGS:    + 16
    ; Return CS:        + 8
    ; Return RIP:       + 0

    ; Restore the register RIP of the next Task onto the stack
    mov rax, [rdi + TaskState_RIP]
    mov [rsp + 0], rax

    ; Restore the register CS of the next Task onto the stack
    mov rax, [rdi + TaskState_CS]
    mov [rsp + 8], rax

    ; Restore the register RFLAGS of the next Task onto the stack
    mov rax, [rdi + TaskState_RFLAGS]
    mov [rsp + 16], rax

    ; Restore the register RSP of the next Task onto the stack
    mov rax, [rdi + TaskState_RSP]
    mov [rsp + 24], rax

    ; Restore the register SS of the next Task onto the stack
    mov rax, [rdi + TaskState_SS]
    mov [rsp + 32], rax

    ; Restore the register RAX register of the next Task
    mov rax, [rdi + TaskState_RAX]

    ; Restore the remaining Segment Registers
    mov ds, [rdi + TaskState_DS]
    mov es, [rdi + TaskState_ES]
    mov fs, [rdi + TaskState_FS]
    mov gs, [rdi + TaskState_GS]

    ; Send the reset signal to the master PIC...
    push rax
    mov rax, 0x20
    out 0x20, eax
    pop rax

    ; Return from the Interrupt Handler
    ; Because we have patched the Stack Frame of the Interrupt Handler, we continue with the execution of 
    ; the next Task - based on the restored register RIP on the Stack...
    iretq

; This function returns a pointer to the Task structure of the current executing Task
GetTaskState:
    mov rax, r15
    ret