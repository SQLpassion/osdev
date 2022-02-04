;************************************************;
; This file contains some helper functions.
;************************************************;

;================================================
; This function prints a whole string, where the 
; input string is stored in the register "SI"
;================================================
PRINTLINE:
    ; Set the TTY mode
    MOV AH, 0xE
    INT 10

    MOV AL, [SI]
    CMP AL, 0
    JE END_PRINTLINE
    
    INT 0x10
    INC SI
    JMP PRINTLINE
    
    END_PRINTLINE:
RET