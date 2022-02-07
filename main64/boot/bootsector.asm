; Tell the Assembler that we are loaded at offset 0x7C00
[ORG 0x7C00]
[BITS 16]

JMP MAIN                    ; Jump over the BPB directly to start of the boot loader
NOP                         ; This padding of 1 additional byte is needed, because the
                            ; BPB starts at offset 0x03 in the boot sector

;*********************************************
;    BIOS Parameter Block (BPB) for FAT12
;*********************************************
bpbOEM                  DB "KAOS    "
bpbBytesPerSector:      DW 512
bpbSectorsPerCluster:   DB 1
bpbReservedSectors:     DW 1
bpbNumberOfFATs:        DB 2
bpbRootEntries:         DW 224
bpbTotalSectors:        DW 2880
bpbMedia:               DB 0xF0
bpbSectorsPerFAT:       DW 9
bpbSectorsPerTrack:     DW 18
bpbHeadsPerCylinder:    DW 2
bpbHiddenSectors:       DD 0
bpbTotalSectorsBig:     DD 0
bsDriveNumber:          DB 0
bsUnused:               DB 0
bsExtBootSignature:     DB 0x29
bsSerialNumber:         DD 0xa0a1a2a3
bsVolumeLabel:          DB "KAOS DRIVE "
bsFileSystem:           DB "FAT12   "

MAIN:
    ; Setup the DS and ES register
    XOR     AX, AX
    MOV     DS, AX
    MOV     ES, AX

    ; Prepare the stack
    ; Otherwise we can't call a function...
    MOV     AX, 0x7000
    MOV     SS, AX
    MOV     BP, 0x8000
    MOV     SP, BP

    ; Print out a welcome message
    MOV     SI, WelcomeMessage
    CALL    PrintLine

    ; Load a file into memory
    CALL    LoadRootDirectory

    ; Print out the content of the loaded file
    MOV     SI, IMAGE_OFFSET
    CALL    PrintLine

    JMP     $ ; Jump to current address = infinite loop

; Include some helper functions
%INCLUDE "../boot/functions.asm"

; OxA: new line
; 0xD: carriage return
; 0x0: null terminated string
WelcomeMessage: DB 'Booting KAOS...', 0xD, 0xA, 0x0

ROOTDIRECTORY_AND_FAT_OFFSET        EQU 0x500
IMAGE_OFFSET                        EQU 0x1200
Sector                              DB 0x00
Head                                DB 0x00
Track                               DB 0x00
FileName                            DB "HELLO   TXT"
FileReadError                       DB 'Failure', 0
Cluster                             DW 0x0000
DiskReadErrorMessage:               DB 'Disk read error...', 0
DataSectorBeginning:                DW 0x0000

; Padding and magic number
TIMES 510 - ($-$$) DB 0
DW 0xAA55