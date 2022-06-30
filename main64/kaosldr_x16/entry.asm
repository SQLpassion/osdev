[ORG 0x1200]
[BITS 16]

MAIN:
    ; Get the current date from the BIOS
    MOV     AH, 0x4
    INT     0x1A
    PUSH    DX
    XOR     AH, AH
    MOV     AL, CH
    CALL    BCD2DEC
    MOV     BX, 100
    MUL     BX
    XOR     CH, CH
    XCHG    CX, AX
    CALL    BCD2DEC
    XCHG    CX, AX
    XOR     CH, CH
    ADD     AX, CX

    ; Print out a welcome message
    MOV     SI, WelcomeMessage
    CALL    PrintLine

    JMP     $

; Include some helper functions
%INCLUDE "functions.asm"                                                                                                                                        

WelcomeMessage: DB 'Executing KAOSLDR_x16.bin...', 0xD, 0xA, 0x0