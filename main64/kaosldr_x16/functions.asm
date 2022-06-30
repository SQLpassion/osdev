;************************************************;
; This file contains some helper functions.
;************************************************;

;================================================
; This function prints a whole string, where the 
; input string is stored in the register "SI"
;================================================
PrintLine:
    ; Set the TTY mode
    MOV     AH, 0xE
    INT     10

    ; Set the input string
    MOV     AL, [SI]
    CMP     AL, 0
    JE      PrintLine_End
    
    INT     0x10
    INC     SI
    JMP     PrintLine
    
    PrintLine_End:
RET

BCD2DEC:
    MOV     CL, AL
    SHR     AL, 4
    MOV     CH, 10
    MUL     CH
    AND     CL, 0Fh
    ADD     AL, CL
RET