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

;************************************************;
; Convert LBA to CHS
; AX: LBA Address to convert
;
; absolute sector = (logical sector / sectors per track) + 1
; absolute head   = (logical sector / sectors per track) MOD number of heads
; absolute track  = logical sector / (sectors per track * number of heads)
;
;************************************************;
LBA2CHS:
    XOR     DX, DX                                  ; prepare dx:ax for operation
    DIV     WORD [bpbSectorsPerTrack]               ; calculate
    INC     DL                                      ; adjust for sector 0
    MOV     BYTE [Sector], DL
    XOR     DX, DX                                  ; prepare dx:ax for operation
    DIV     WORD [bpbHeadsPerCylinder]              ; calculate
    MOV     BYTE [Head], DL
    MOV     BYTE [Track], AL

; Return...
RET

;************************************************;
; Converts a FAT Cluster to LBA.
; We have to substract 2 from the FAT cluster, because the first 2
; FAT clusters have a special purpose, and they have no
; corresponding data cluster in the file
;
; LBA = (FAT Cluster - 2) * sectors per cluster
;************************************************;
FATCluster2LBA:
    SUB     AX, 0x0002                              ; zero base cluster number
    XOR     CX, CX
    MOV     CL, BYTE [bpbSectorsPerCluster]         ; convert byte to word
    MUL     CX
    ADD     AX, WORD [DataSectorBeginning]          ; base data sector

; Return...
RET

;======================================================
; Loads data from the disk
; dh: number of the sectors we want to read
; cl: number of the sector were we will start to read
;======================================================
LoadSectors:
    PUSH    DX

    MOV     AH, 0x02                                ; BIOS read selector function
    MOV     AL, DH                                  ; Number of the sector we want to read
    MOV     CH, BYTE [Track]                        ; Track
    MOV     CL, BYTE [Sector]                       ; Sector
    MOV     DH, BYTE [Head]                         ; Head
    MOV     DL, 0                                   ; Select the boot drive
    INT     0x13                                    ; BIOS interrupt that triggers the I/O

    JC      DiskError                               ; Error handling

    POP     DX
    CMP     DH, AL                                  ; Do we have read the amount of sectors that we have expected
    JNE     DiskError

; Return...
RET

;=============================================
DiskError:
    MOV     SI, DiskReadErrorMessage
    CALL    PrintLine

    ; Endless loop
    JMP     $

Failure:
    MOV     SI, FileReadError
    CALL    PrintLine

    ; Endless loop
    JMP     $

;=========================================================
; Loads the FAT12 Root Directory, and loads a given file
; into memory.
;=========================================================
LoadRootDirectory:
    ; In the first step we calculate the size (number of sectors) 
    ; of the root directory and store the value in the CX register
    ; Calculation: 32 * bpbRootEntries / bpbBytesPerSector
    XOR     CX, CX
    XOR     DX, DX
    MOV     AX, 0x0020                              ; 32 byte directory entry
    MUL     WORD [bpbRootEntries]                   ; total size of directory
    DIV     WORD [bpbBytesPerSector]                ; sectors used by directory
    XCHG    AX, CX
          
    ; In the next step we calculate the LBA address (number of the sector)
    ; of the root directory and store the location in the AX register
    ; AX holds afterwards an LBA address, which must be converted to a CHS address
    ;
    ; Calcuation: bpbNumberOfFATs * bpbSectorsPerFAT + bpbReservedSectors
    MOV     AL, BYTE [bpbNumberOfFATs]              ; Number of FATs
    MUL     WORD [bpbSectorsPerFAT]                 ; Number of sectors used by the FATs
    ADD     AX, WORD [bpbReservedSectors]           ; Add the boot sector (and reserved sectors, if available)

    ; Calculate the address where the first cluster of data begins
    ; Calculation: Root Directory Size (register AX) + (size of FATs + boot sector + reserved sectors [register CX])
    MOV     WORD [DataSectorBeginning], AX          ; Size of the root directory
    ADD     WORD [DataSectorBeginning], CX          ; FAT sectors + boot sector + reserved sectors

    ; Convert the calculated LBA address (stored in AX) to a CHS address
    CALL    LBA2CHS

    ; And finally we read the complete root directory into memory
    MOV     BX, ROOTDIRECTORY_AND_FAT_OFFSET        ; Load the Root Directory at 0x500
    MOV     DH, CL                                  ; Load the number of sectors stored in CX
    CALL    LoadSectors                             ; Perform the I/O operation

    ; Now we have to find our file in the Root Directory
    MOV     CX, [bpbRootEntries]                    ; The number of root directory entries
    MOV     DI, ROOTDIRECTORY_AND_FAT_OFFSET        ; Address of the Root directory
    .Loop:
        PUSH    CX
        MOV     CX, 11                              ; We compare 11 characters (8.3 convention)
        MOV     SI, FileName                        ; Compare against the file name
        PUSH    DI
    REP CMPSB                                       ; Test for string match
        POP     DI
        JE      LoadFAT                             ; When we have a match, we load the FAT
        POP     CX
        ADD     DI, 32                              ; When we don't have a match, we go to next root directory entry (+ 32 bytes)
        LOOP    .Loop
        JMP     Failure                             ; The file image wasn't found in the root directory

LoadFAT:
    MOV     DX, WORD [DI + 0x001A]                  ; Add 26 bytes to the current entry of the root directory, so that we get the start cluster
    MOV     WORD [Cluster], DX                      ; Store the 2 bytes of the start cluster (byte 26 & 27 of the root directory entry) in the variable "cluster"

    ; Calculate the number of sectors used by all FATs (bpbNumberOfFATs * bpbSectorsPerFAT)
    XOR     AX, AX
    MOV     BYTE [Track], AL                        ; Initialize the track with 0
    MOV     BYTE [Head], AL                         ; Initialize the head with 0
    MOV     AL, 1                                   ; We just read 1 FAT, so that we stay within the 1st track
    MUL     WORD [bpbSectorsPerFAT]                 ; The sectors per FAT
    MOV     DH, AL                                  ; Store the number of sectors for all FATs in register DX
    
    ; Load the FAT into memory
    MOV     BX, ROOTDIRECTORY_AND_FAT_OFFSET        ; Offset in memory at which we want to load the FATs
    MOV     CX, WORD [bpbReservedSectors]           ; Number of the reserved sectors (1)
    ADD     CX, 1                                   ; Add 1 to the number of reserved sectors, so that our start sector is the 2nd sector (directly after the boot sector)
    MOV     BYTE [Sector], CL                       ; Sector where we start to read
    CALL    LoadSectors                             ; Call the load routine
    MOV     BX, WORD [Loader_Offset]                ; Address where the first cluster should be stored
    PUSH    BX                                      ; Store the current kernel address on the stack

LoadImage:
    MOV     AX, WORD [Cluster]                      ; FAT cluster to read
    CALL    FATCluster2LBA                          ; Convert the FAT cluster to LBA (result stored in AX)
    
    ; Convert the calculated LBA address (input in AX) to a CHS address
    CALL    LBA2CHS

    XOR     DX, DX
    MOV     DH, BYTE [bpbSectorsPerCluster]         ; Number of the sectors we want to read
    POP     BX                                      ; Get the current kernel address from the stack (for every sector we read, we advance the address by 512 bytes)
    CALL    LoadSectors                             ; Read the cluster into memory
    ADD     BX, 0x200                               ; Advance the kernel address by 512 bytes (1 sector that was read from disk)
    PUSH    BX

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
    JNZ     LoadRootDirectory_OddCluster
          
LoadRootDirectory_EvenCluster:
    AND     DX, 0000111111111111b                   ; Take the lowest 12 bits
    JMP     LoadRootDirectory_Done
         
LoadRootDirectory_OddCluster:
    SHR     DX, 0x0004                              ; Take the highest 12 bits
          
LoadRootDirectory_Done:
    MOV     WORD [Cluster], DX                      ; store new cluster
    CMP     DX, 0x0FF0                              ; Test for end of file
    JB      LoadImage

LoadRootDirectory_End:
    ; Restore the stack, so that we can do a RET
    POP     BX
    POP     BX

; Return...
RET