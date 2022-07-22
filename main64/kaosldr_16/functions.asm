;************************************************;
; This file contains some helper functions.
;************************************************;

; This structure stores all the information that we retrieve from the BIOS while we are in x16 Real Mode
STRUC BiosInformationBlock
    .Year:      RESD 1
    .Month:     RESW 1
    .Day:       RESW 1
    .Hour:      RESW 1
    .Minute:    RESW 1
    .Second:    RESW 1
ENDSTRUC

;================================================
; This function prints a whole string, where the 
; input string is stored in the register "SI"
;================================================
PrintString:
    ; Set the TTY mode
    MOV     AH, 0xE
    INT     10

    ; Set the input string
    MOV     AL, [SI]
    CMP     AL, 0
    JE      PrintString_End
    
    INT     0x10
    INC     SI
    JMP     PrintString
    
    PrintString_End:
RET

;================================================
; This function prints out a decimal number
; that is stored in the register AX.
;================================================
PrintDecimal:
    MOV     CX, 0
    MOV     DX, 0

PrintDecimal_Start:
    CMP     AX ,0
    JE      PrintDecimal_Print
    MOV     BX, 10
    DIV     BX
    PUSH    DX
    INC     CX
    XOR     DX, DX
    JMP     PrintDecimal_Start
PrintDecimal_Print:
    CMP     CX, 0
    JE      PrintDecimal_Exit
    POP     DX
        
    ; Add 48 so that it represents the ASCII value of digits
    MOV     AL, DL
    ADD     AL, 48
    MOV     AH, 0xE
    INT     0x10

    DEC     CX
    JMP     PrintDecimal_Print
PrintDecimal_Exit:
RET

;================================================
; This function converts a BCD number to a
; decimal number.
;================================================
Bcd2Decimal:
    MOV     CL, AL
    SHR     AL, 4
    MOV     CH, 10
    MUL     CH
    AND     CL, 0Fh
    ADD     AL, CL
RET

;=================================================
; This function retrieves the date from the BIOS.
;=================================================
GetDate:
    ; Get the current date from the BIOS
    MOV     AH, 0x4
    INT     0x1A

    ; Century
    PUSH    CX
    MOV     AL, CH
    CALL    Bcd2Decimal
    MOV     [Year1], AX
    POP     CX

    ; Year
    MOV     AL, CL
    CALL    Bcd2Decimal
    MOV     [Year2], AX

    ; Month
    MOV     AL, DH
    CALL    Bcd2Decimal 
    MOV     WORD [ES:DI + BiosInformationBlock.Month], AX

    ; Day
    MOV     AL, DL
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Day], AX

    ; Calculate the whole year (e.g. "20" * 100 + "22" = 2022)
    MOV     AX, [Year1]
    MOV     BX, 100
    MUL     BX
    MOV     BX, [Year2]
    ADD     AX, BX
    MOV     WORD [ES:DI + BiosInformationBlock.Year], AX
RET

;=================================================
; This function retrieves the time from the BIOS.
;=================================================
GetTime:
    ; Get the current time from the BIOS
    MOV     AH, 0x2
    INT     0x1A

    ; Hour
    PUSH    CX
    MOV     AL, CH
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Hour], AX
    POP     CX

    ; Minute
    MOV     AL, CL
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Minute], AX

    ; Second
    MOV     AL, DH
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Second], AX
RET

;=============================================
; This function enables the A20 gate
;=============================================
EnableA20:
	CLI					; Disables interrupts
	PUSH	AX			; Save AX on the stack
	MOV     AL, 2
	OUT     0x92, AL
	POP	    AX			; Restore the value of AX from the stack
	STI					; Enable the interrupts again
RET 