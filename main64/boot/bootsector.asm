; Tell the Assembler that the boot sector is loaded at the offset 0x7C00
[ORG 0x7C00]
[BITS 16]

JMP Main                    ; Jump over the BPB directly to start of the boot loader
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

Main:
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

    ; Print out a boot message
    MOV     SI, BootMessage
    CALL    PrintLine

    ; Load the KAOSLDR.BIN file into memory
    MOV     CX, 11
    LEA     SI, [SecondStageFileName]
    LEA     DI, [FileName]
    REP     MOVSB
    CALL    LoadFileIntoMemory

    ; Execute the loaded loader...
    CALL     KAOSLDR_OFFSET

; Include some helper functions
%INCLUDE "../boot/functions.asm"

; OxA: New Line
; 0xD: Carriage Return
; 0x0: Null Terminated String
BootMessage: DB 'Booting KAOS...', 0xD, 0xA, 0x0
ROOTDIRECTORY_AND_FAT_OFFSET        EQU 0x500
KAOSLDR_OFFSET                      EQU 0x2000
Loader_Offset                       DW 0x0000
FileName                            DB 11 DUP (" ")
SecondStageFileName                 DB "KAOSLDR BIN"
FileReadError                       DB 'file not found...', 0
Cluster                             DW 0x0000
CRLF:                               DB 0xD, 0xA, 0x0

; Padding and magic number
TIMES 510 - ($-$$) DB 0
DW 0xAA55