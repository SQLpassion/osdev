; Tell the Assembler that the boot sector is loaded at the offset 0x7C00
[ORG 0x7C00]
[BITS 16]

JMP Main                    ; Jump over the BPB directly to start of the boot loader
NOP                         ; This padding of 1 additional byte is needed, because the
                            ; BPB starts at offset 0x03 in the boot sector

;*********************************************
;    BIOS Parameter Block (BPB) for FAT32
;*********************************************
; IMPORTANT: These BPB values are only PLACEHOLDERS. The build script formats the
; image with `mformat` (which writes the authoritative FAT32 BPB) and then overlays
; only this boot sector's code region (offset 0x5A onwards) plus the JMP/OEM bytes
; (0x00..0x0B), restoring mformat's BPB fields (0x0B..0x5A) afterwards. The only
; purpose of these `DB`/`DW` directives is to place `Main:` at exactly offset 0x5A.
; The boot code itself never reads the BPB; it loads the loaders from fixed reserved
; sectors. Only the later stages (kaosldr_64, kernel) parse the (mformat) BPB.
bpbOEM                  DB "KAOS    "            ; 0x03
bpbBytesPerSector:      DW 512                   ; 0x0B
bpbSectorsPerCluster:   DB 1                     ; 0x0D
bpbReservedSectors:     DW 64                    ; 0x0E
bpbNumberOfFATs:        DB 2                     ; 0x10
bpbRootEntries:         DW 0                     ; 0x11 (0 for FAT32)
bpbTotalSectors:        DW 0                     ; 0x13 (0 for FAT32, see TotalSectorsBig)
bpbMedia:               DB 0xF8                  ; 0x15 (fixed disk)
bpbSectorsPerFAT:       DW 0                     ; 0x16 (0 for FAT32, see SectorsPerFAT32)
bpbSectorsPerTrack:     DW 63                    ; 0x18
bpbHeadsPerCylinder:    DW 255                   ; 0x1A
bpbHiddenSectors:       DD 0                     ; 0x1C
bpbTotalSectorsBig:     DD 0                     ; 0x20
; --- FAT32 extended BPB ---
bpbSectorsPerFAT32:     DD 0                     ; 0x24
bpbExtFlags:            DW 0                     ; 0x28
bpbFSVersion:           DW 0                     ; 0x2A
bpbRootCluster:         DD 2                     ; 0x2C
bpbFSInfoSector:        DW 1                     ; 0x30
bpbBackupBootSector:    DW 6                     ; 0x32
bpbReserved:            TIMES 12 DB 0            ; 0x34
bsDriveNumber:          DB 0x80                  ; 0x40
bsReserved1:            DB 0                     ; 0x41
bsExtBootSignature:     DB 0x29                  ; 0x42
bsSerialNumber:         DD 0xa0a1a2a3            ; 0x43
bsVolumeLabel:          DB "KAOS DRIVE "         ; 0x47
bsFileSystem:           DB "FAT32   "            ; 0x52 .. 0x5A

Main:                                            ; must reside at offset 0x5A
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

    ; Load the KLDR64.BIN file from its fixed reserved sectors into memory.
    ; The loaders live in the FAT32 reserved-sector region at fixed LBAs, so the
    ; boot sector needs no filesystem knowledge: it just reads fixed sector ranges.
    MOV     BX, KLDR64_MAX_SECTORS
    MOV     ECX, KLDR64_LBA
    MOV     EDI, KAOSLDR64_OFFSET
    CALL    ReadSector

    ; Load the KLDR16.BIN file from its fixed reserved sectors into memory.
    ; KLDR16 is read last (into 0x2000) and then executed; it expects KLDR64 to
    ; already reside at 0x3000.
    MOV     BX, KLDR16_MAX_SECTORS
    MOV     ECX, KLDR16_LBA
    MOV     EDI, KAOSLDR16_OFFSET
    CALL    ReadSector

    ; Execute the KLDR16.BIN file...
    CALL KAOSLDR16_OFFSET

; Include some helper functions
%INCLUDE "../boot/functions.asm"

; OxA: New Line
; 0xD: Carriage Return
; 0x0: Null Terminated String
BootMessage: DB 'Booting KAOS...', 0xD, 0xA, 0x0

; Destination addresses (low memory) for the two early loaders.
KAOSLDR16_OFFSET                    EQU 0x2000
KAOSLDR64_OFFSET                    EQU 0x3000

; Fixed location and maximum size of each loader inside the FAT32 reserved-sector
; region. These MUST match the build script (RESERVED_SECTORS / *_LBA / *_MAX_SECTORS).
; KLDR16 occupies LBA 8..15, KLDR64 occupies LBA 16..47; FSInfo (LBA 1) and the
; backup boot sector (LBA 6) stay untouched. The FAT starts at LBA 64.
;
; CRITICAL: the loaders are read into low memory, and this very boot sector executes
; at 0x7C00. KLDR64 is read to 0x3000, so its sector count MUST keep
; 0x3000 + KLDR64_MAX_SECTORS*512 <= 0x7C00 (i.e. <= 38), otherwise the read would
; overwrite the running boot sector. 32 sectors (ends at 0x7000) leaves a safe margin
; while giving ample headroom for the (currently ~10-sector) loader. Likewise KLDR16
; at 0x2000 must stay below KLDR64 at 0x3000 (<= 8 sectors).
KLDR16_LBA                          EQU 8
KLDR16_MAX_SECTORS                  EQU 8
KLDR64_LBA                          EQU 16
KLDR64_MAX_SECTORS                  EQU 32

; Padding and magic number
TIMES 510 - ($-$$) DB 0
DW 0xAA55
