; Tell the Assembler that the boot sector is loaded at the offset 0x7C00
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

    ; Print out a boot message
    MOV     SI, BootMessage
    CALL    PrintLine

    ; Load the KAOSLDR.BIN file into memory
    MOV     CX, 11
    LEA     SI, [SecondStageFileName]
    LEA     DI, [FileName]
    REP     MOVSB
    MOV     WORD [Loader_Offset], KAOSLDR_OFFSET
    CALL    LoadRootDirectory

    ; Execute the KAOSLDR.BIN file...
    CALL KAOSLDR_OFFSET

; Include some helper functions
%INCLUDE "../boot/functions.asm"

; OxA: New Line
; 0xD: Carriage Return
; 0x0: Null Terminated String
BootMessage: DB 'Booting KAOS...', 0xD, 0xA, 0x0

ROOTDIRECTORY_AND_FAT_OFFSET        EQU 0x500
KAOSLDR_OFFSET                      EQU 0x2000
Loader_Offset                       DW 0x0000
Sector                              DB 0x00
Head                                DB 0x00
Track                               DB 0x00
FileName                            DB 11 DUP (" ")
SecondStageFileName                 DB "KAOSLDR BIN"
FileReadError                       DB 'Failure', 0
Cluster                             DW 0x0000
DiskReadErrorMessage:               DB 'Disk Error', 0
DataSectorBeginning:                DW 0x0000

;===================================================================
; Definition of the GDT, needed for entering the x32 Protected Mode
; More information: https://wiki.osdev.org/Global_Descriptor_Table
;===================================================================
GDT_START:

; Null Descriptor
GDT_NULL:
DD          0x0
DD          0x0

; Code Segment Descriptor
GDT_CODE:
DW          0xFFFF                  ; Limit: 2 bytes
DW          0x0                     ; Base:  2 bytes
DB          0x0                     ; Base:  1 byte
DB          10011010b               ; Access Byte:
                                    ;   - Bit 7: Present
                                    ;   - Bit 6: Privilege Level
                                    ;   - Bit 5: Privilege Level
                                    ;   - Bit 4: Descriptor Type Bit
                                    ;   - Bit 3: Executable Bit
                                    ;   - Bit 2: Direction Bit
                                    ;   - Bit 1: Readable/Writeable Bit
                                    ;   - Bit 0: Accessed Bit  
DB          11001111b               ; Flags (4 bits) + Limits (4 Bits)
                                    ;   - Bit 7: Granularity Flag
                                    ;   - Bit 6: Size Flag
                                    ;   - Bit 5: Long-Mode Code Flag
                                    ;   - Bit 4: Reserved
                                    ;   - Bit 3 - 0: Limit
DB          0x0                     ; Base: 1 byte

; Data Segment Descriptor
GDT_DATA:
DW          0xFFFF                  ; Limit: 2 bytes
DW          0x0                     ; Base:  2 bytes
DB          0x0                     ; Base:  1 byte
DB          10010010b               ; Access Byte:
                                    ;   - Bit 7: Present
                                    ;   - Bit 6: Privilege Level
                                    ;   - Bit 5: Privilege Level
                                    ;   - Bit 4: Descriptor Type Bit
                                    ;   - Bit 3: Executable Bit
                                    ;   - Bit 2: Direction Bit
                                    ;   - Bit 1: Readable/Writeable Bit
                                    ;   - Bit 0: Accessed Bit  
DB          11001111b               ; Flags (4 bits) + Limits (4 Bits)
                                    ;   - Bit 7: Granularity Flag
                                    ;   - Bit 6: Size Flag
                                    ;   - Bit 5: Long-Mode Code Flag
                                    ;   - Bit 4: Reserved
                                    ;   - Bit 3 - 0: Limit
DB          0x0                     ; Base: 1 byte

GDT_END:

;==================================
; Definition of the GDT Descriptor
;==================================
GDT_DESCRIPTOR:
DW GDT_END - GDT_START - 1	        ; Size of the GDT - 1
DD GDT_START				        ; Start address of the GDT
                                    ; This address will be loaded into the GDT register

CODE_SEGMENT EQU GDT_CODE - GDT_START
DATA_SEGMENT EQU GDT_DATA - GDT_START

; Padding and magic number
TIMES 510 - ($-$$) DB 0
DW 0xAA55