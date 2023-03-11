[GLOBAL GdtFlush]

; Loads the GDT table
GdtFlush:
    CLI

    ; The first function parameter is provided in the RDI register on the x64 architecture
    ; RDI points to the variable gdtPointer (defined in the C code)
    LGDT [RDI]

    ; Load the TSS
    MOV AX, 0x2B ; This is the 6th entry from the GDT with the requested RPL of 3
    LTR AX

    STI
    RET