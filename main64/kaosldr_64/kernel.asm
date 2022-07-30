[BITS 64]
[GLOBAL ExecuteKernel]

; Executes the loaded x64 OS Kernel
ExecuteKernel:
    ; Make a call to the memory location where the Kernel was loaded...
    MOV     RAX, QWORD 0xFFFF800000100000
    CALL    RAX