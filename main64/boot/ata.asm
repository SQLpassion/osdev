;************************************************
; This file contains ATA PIO functions.
;************************************************

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

;================================================
; This function checks the ATA PIO BSY flag.
;================================================
Check_ATA_BSY:
    MOV     DX, 0x1F7
    IN      AL, DX
    TEST    AL, 0x80
    JNZ     Check_ATA_BSY
RET

Check_ATA_RDY:
    MOV     DX, 0x1F7
    IN      AL, DX
    TEST    AL, 0x40
    JZ      Check_ATA_RDY
RET

ReadSector:
    ; Sector count
    MOV     DX, 0x1F2
    MOV     AL, 1       ; Sector Count
    OUT     DX, AL

    ; LBA
    MOV     DX, 0x1F3
    MOV     AL, 1       ; LBA
    OUT     DX, AL

    ; Read Command
    MOV     DX, 0x1F7
    MOV     AL, 0x20    ; Read Command
    OUT     DX, AL

    CALL    Check_ATA_BSY
    CALL    Check_ATA_RDY

    MOV     EDI, 0x2000
    MOV     DX, 0x1F0
    MOV     CX, 256
    REP     INSW
RET