[ORG 0x1200]
[BITS 16]

MAIN:
    ; Print out a welcome message
    MOV     SI, WelcomeMessage
    CALL    PrintLine

; Include some helper functions
%INCLUDE "functions.asm"

WelcomeMessage: DB 'Executing KAOSLDR_x16.bin...', 0xD, 0xA, 0x0