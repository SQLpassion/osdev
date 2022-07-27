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

;================================================
; This function checks the ATA PIO RDY flag.
;================================================
Check_ATA_RDY:
    MOV     DX, 0x1F7
    IN      AL, DX
    TEST    AL, 0x40
    JZ      Check_ATA_RDY
RET

;================================================
; This function reads a sector through ATA PIO.
; BX:  Nunber of sectors to read
; ECX: Starting LBA
; EDI: Destination Address
;================================================
ReadSector:
    ; Sector count
    MOV     DX, 0x1F2
    MOV     AL, BL
    OUT     DX, AL

    ; LBA - Low Byte
    MOV     DX, 0x1F3
    MOV     AL, CL
    OUT     DX, AL

    ; LBA - Middle Byte
    MOV     DX, 0x1F4
    MOV     AL, CH
    OUT     DX, AL

    ; LBA - High Byte
    BSWAP   ECX
    MOV     DX, 0x1F5
    MOV     AL, CH
    OUT     DX, AL

    ; Read Command
    MOV     DX, 0x1F7
    MOV     AL, 0x20    ; Read Command
    OUT     DX, AL

    .ReadNextSector:
        CALL    Check_ATA_BSY
        CALL    Check_ATA_RDY

        ; Read the sector of 512 bytes into ES:EDI
        ; EDI is incremented by 512 bytes automatically
        MOV     DX, 0x1F0
        MOV     CX, 256
        REP     INSW

        ; Decrease the number of sectors to read and compare it to 0
        DEC     BX
        CMP     BX, 0
        JNE     .ReadNextSector
RET