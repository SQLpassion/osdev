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
; This function prints out a decimal number
; that is stored in the register AX.
;================================================
PrintDecimal:
    MOV     CX, 0
    MOV     DX, 0

.PrintDecimal_Start:
    CMP     AX ,0
    JE      .PrintDecimal_Print
    MOV     BX, 10
    DIV     BX
    PUSH    DX
    INC     CX
    XOR     DX, DX
    JMP     .PrintDecimal_Start
.PrintDecimal_Print:
    CMP     CX, 0
    JE      .PrintDecimal_Exit
    POP     DX
        
    ; Add 48 so that it represents the ASCII value of digits
    MOV     AL, DL
    ADD     AL, 48
    MOV     AH, 0xE
    INT     0x10

    DEC     CX
    JMP     .PrintDecimal_Print
.PrintDecimal_Exit:
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
; This function checks the ATA PIO DRQ flag.
;================================================
Check_ATA_DRQ:
    MOV     DX, 0x1F7
    IN      AL, DX
    TEST    AL, 0x08
    JZ      Check_ATA_DRQ
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
        CALL    Check_ATA_DRQ

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

;=================================
; Loads a given file into memory.
;=================================
LoadFileIntoMemory:
    .LoadRootDirectory:
        ; Load the Root Directory into memory.
        ; It starts at the LBA 19, and consists of 14 sectors.
        MOV     BL,  0xE                                ; 14 sectors to be read
        MOV     ECX, 0x13                               ; The LBA is 19
        MOV     EDI, ROOTDIRECTORY_AND_FAT_OFFSET       ; Destination address
        CALL    ReadSector                              ; Loads the complete Root Directory into memory

    .FindFileInRootDirectory:
        ; Now we have to find our file in the Root Directory
        MOV     CX, [bpbRootEntries]                    ; The number of root directory entries
        MOV     DI, ROOTDIRECTORY_AND_FAT_OFFSET        ; Address of the Root directory
        .Loop:
            PUSH    CX
            MOV     CX, 11                              ; We compare 11 characters (8.3 convention)
            MOV     SI, FileName                        ; Compare against the file name
            PUSH    DI
            REP     CMPSB                               ; Test for string match

            POP     DI
            JE      .LoadFAT                            ; When we have a match, we load the FAT
            POP     CX
            ADD     DI, 32                              ; When we don't have a match, we go to next root directory entry (+ 32 bytes)
            LOOP    .Loop
            JMP     Failure                             ; The file image wasn't found in the root directory

    .LoadFAT:
        ; Store the first FAT cluster of the file to be read in the variable "Cluster"
        MOV     DX, WORD [DI + 0x001A]              ; Add 26 bytes to the current entry of the root directory, so that we get the start cluster
        MOV     WORD [Cluster], DX                  ; Store the 2 bytes of the start cluster (byte 26 & 27 of the root directory entry) in the variable "cluster"

        ; Load the FATs into memory.
        ; It starts at the LBA 1 (directly after the boot sector), and consists of 18 sectors (2 x 9).
        MOV     BL, 0x12                                ; 18 sectors to be read
        MOV     ECX, 0x1                                ; The LBA is 1
        MOV     EDI, ROOTDIRECTORY_AND_FAT_OFFSET       ; Offset in memory at which we want to load the FATs
        CALL    ReadSector                              ; Call the load routine
        MOV     EDI, [Loader_Offset]                    ; Address where the first cluster should be stored

    .LoadImage:
        ; Print out the current offset where the cluster is loaded into memory
        MOV     AX, DI
        CALL    PrintDecimal
        MOV     SI, CRLF
        CALL    PrintLine

        ; Load the first sector of the file into memory
        MOV     AX, WORD [Cluster]                      ; First FAT cluster to read
        ADD     AX, 0x1F                                ; Add 31 sectors to the retrieved FAT cluster to get the LBA address of the first FAT cluster
        MOV     ECX, EAX                                ; LBA
        MOV     BL, 1                                   ; 1 sector to be read
        CALL    ReadSector                              ; Read the cluster into memory
        
        ; Compute the next cluster that we have to load from disk
        MOV     AX, WORD [Cluster]                      ; identify current cluster
        MOV     CX, AX                                  ; copy current cluster
        MOV     DX, AX                                  ; copy current cluster
        SHR     DX, 0x0001                              ; divide by two
        ADD     CX, DX                                  ; sum for (3/2)
        MOV     BX, ROOTDIRECTORY_AND_FAT_OFFSET        ; location of FAT in memory
        ADD     BX, CX                                  ; index into FAT
        MOV     DX, WORD [BX]                           ; read two bytes from FAT
        TEST    AX, 0x0001
        JNZ     .LoadRootDirectoryOddCluster
          
    .LoadRootDirectoryEvenCluster:
        AND     DX, 0000111111111111b                   ; Take the lowest 12 bits
        JMP     .LoadRootDirectoryDone
            
    .LoadRootDirectoryOddCluster:
        SHR     DX, 0x0004                              ; Take the highest 12 bits
            
    .LoadRootDirectoryDone:
        MOV     WORD [Cluster], DX                      ; store new cluster
        CMP     DX, 0x0FF0                              ; Test for end of file
        JB      .LoadImage

    .LoadRootDirectoryEnd:
        ; Restore the stack, so that we can do a RET
        POP     BX
RET

Failure:
    MOV     SI, FileReadError
    CALL    PrintLine

    ; Endless loop
    JMP     $