; Tell the Assembler that KLDR16.BIN is loaded at the offset 0x2000
[ORG 0x2000]
[BITS 16]

Main:
    ; Get the current date from the BIOS
    MOV	    DI, BIB_OFFSET
    CALL    GetDate

    ; Get the current time from the BIOS
    CALL    GetTime

    ; Get the Memory Map from the BIOS
    CALL    GetMemoryMap
    
    ; Enables the A20 gate
    CALL    EnableA20

     ; Print out a boot message
    MOV     SI, BootMessage
    CALL    PrintString

    ; Switch to x64 Long Mode and and execute the KLDR64.BIN file
    CALL    SwitchToLongMode

    RET

; Include some helper functions
%INCLUDE "functions.asm"
%INCLUDE "longmode.asm"                                                                                                                               

BIB_OFFSET      EQU 0x1000  ; BIOS Information Block
MEM_OFFSET      EQU 0X1200  ; Memmory Map
Year1           DW 0x00
Year2           DW 0x00
BootMessage:    DB 'Booting KLDR16.BIN...', 0xD, 0xA, 0x0