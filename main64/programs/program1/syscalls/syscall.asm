[BITS 64]
[GLOBAL SYSCALLASM1]
[GLOBAL SYSCALLASM2]
[GLOBAL SYSCALLASM3]

; Raises a SysCall
SYSCALLASM1:
    INT     0x80
    RET

; Raises a SysCall
SYSCALLASM2:
    INT     0x80
    RET

; Raises a SysCall
SYSCALLASM3:
    INT     0x80
    RET