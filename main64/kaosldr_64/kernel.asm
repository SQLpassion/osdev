[BITS 64]
[GLOBAL ExecuteKernel]

; C Declaration: void ExecuteKernel(int KernelSize);
; Executes the loaded x64 OS Kernel
ExecuteKernel:
    MOV     R15, QWORD 0xFFFF800000110000
    
    ; Make a call to the memory location where the Kernel was loaded...
    ; The register RDI contains the size of the loaded Kernel in bytes.
    ; This information will be passed as the first input parameter to the
    ; startup function of the KERNEL.BIN file.
    MOV     RAX, QWORD 0xFFFF800000100000
    CALL    RAX