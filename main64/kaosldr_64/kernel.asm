[BITS 64]
[GLOBAL ExecuteKernel]

; Executes the loaded x64 OS Kernel
ExecuteKernel:
    ; Jump to the memory location where the Kernel was loaded...
    MOV RAX, QWORD 0x100000
    JMP RAX