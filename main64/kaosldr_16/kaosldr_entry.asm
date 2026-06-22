; Tell the Assembler that KLDR16.BIN is loaded at the offset 0x2000
[ORG 0x2000]
[BITS 16]

Main:
    ; Setup segment registers DS and ES to 0
    XOR     AX, AX
    MOV     DS, AX
    MOV     ES, AX

    ; Get the current date from the BIOS
    MOV	    DI, BIB_OFFSET
    CALL    GetDate

    ; Get the current time from the BIOS
    CALL    GetTime

    ; Get the Memory Map from the BIOS
    CALL    GetMemoryMap
    
    ; Enables the A20 gate
    CALL    EnableA20

    ; Prompt user to select video mode (VGA Text vs VBE Graphics)
    CALL    SelectVideoMode

    ; Print out a boot message
    MOV     SI, BootMessage
    CALL    PrintString

    ; Switch to x64 Long Mode and and execute the KLDR64.BIN file
    CALL    SwitchToLongMode

    RET

SelectVideoMode:
    ; Setup ES to 0 to access BiosInformationBlock and BIOS data
    XOR     AX, AX
    MOV     ES, AX

    ; Print out menu
    MOV     SI, MenuMessage
    CALL    PrintString

.WaitKey:
    ; 1. Check keyboard buffer (non-blocking)
    MOV     AH, 0x01
    INT     0x16
    JZ      .CheckSerial

    ; Key is pressed! Read it (INT 0x16, AH=0x00)
    MOV     AH, 0x00
    INT     0x16
    CMP     AL, '1'
    JE      .SelectVga
    CMP     AL, '2'
    JE      .SelectVbe

.CheckSerial:
    ; 2. Check serial port COM1 (non-blocking)
    ; Line Status Register (LSR) at port 0x3FD. Bit 0 is Data Ready (DR).
    MOV     DX, 0x3FD
    IN      AL, DX
    TEST    AL, 0x01
    JZ      .WaitKey

    ; Read byte from serial port (0x3F8)
    MOV     DX, 0x3F8
    IN      AL, DX
    CMP     AL, '1'
    JE      .SelectVga
    CMP     AL, '2'
    JE      .SelectVbe
    JMP     .WaitKey

.SelectVga:
    ; Set video_type in BIB to 0 (VgaText)
    MOV     DI, BIB_OFFSET
    MOV     DWORD [ES:DI + BiosInformationBlock.VideoType], 0
    ; Zero out the framebuffer fields
    MOV     DWORD [ES:DI + BiosInformationBlock.FbBaseAddress], 0
    MOV     DWORD [ES:DI + BiosInformationBlock.FbBaseAddress + 4], 0
    MOV     DWORD [ES:DI + BiosInformationBlock.FbSize], 0
    MOV     DWORD [ES:DI + BiosInformationBlock.FbSize + 4], 0
    MOV     DWORD [ES:DI + BiosInformationBlock.FbWidth], 0
    MOV     DWORD [ES:DI + BiosInformationBlock.FbHeight], 0
    MOV     DWORD [ES:DI + BiosInformationBlock.FbPixelsPerScanline], 0
    RET

.SelectVbe:
    CALL    SetupVbe
    JC      .VbeFailed
    RET

.VbeFailed:
    ; If VBE failed, fallback to VGA Text Mode
    MOV     SI, VbeFailedMessage
    CALL    PrintString
    JMP     .SelectVga

; SetupVbe returns Carry set on error
SetupVbe:
    ; VBE Controller Info (check if VBE exists)
    MOV     AX, 0x4F00
    MOV     DI, 0x8000
    INT     0x10
    CMP     AX, 0x004F
    JNE     .Error

    ; We query mode info for 1024x768x32 first (mode 0x143)
    MOV     CX, 0x143
    MOV     AX, 0x4F01
    MOV     DI, 0x8000
    INT     0x10
    CMP     AX, 0x004F
    JE      .CheckMode

    ; Fallback to 1024x768x24/32 (mode 0x118)
    MOV     CX, 0x118
    MOV     AX, 0x4F01
    MOV     DI, 0x8000
    INT     0x10
    CMP     AX, 0x004F
    JNE     .Error

.CheckMode:
    ; Check if the mode supports linear framebuffer (bit 7 of ModeAttributes)
    MOV     AX, [0x8000]
    AND     AX, 0x0080
    JZ      .Error

    ; Check BitsPerPixel (must be 24 or 32)
    MOV     AL, [0x8000 + 25]
    CMP     AL, 32
    JE      .ModeOk
    CMP     AL, 24
    JNE     .Error

.ModeOk:
    ; Set VBE mode: BX = mode | 0x4000 (linear framebuffer)
    MOV     AX, 0x4F02
    MOV     BX, CX
    OR      BX, 0x4000
    INT     0x10
    CMP     AX, 0x004F
    JNE     .Error

    ; Mode successfully set! Fill BiosInformationBlock
    MOV     DI, BIB_OFFSET
    MOV     DWORD [ES:DI + BiosInformationBlock.VideoType], 1

    ; PhysBasePtr at offset 40 (dword)
    MOV     EAX, [0x8000 + 40]
    MOV     [ES:DI + BiosInformationBlock.FbBaseAddress], EAX
    MOV     DWORD [ES:DI + BiosInformationBlock.FbBaseAddress + 4], 0

    ; XResolution at offset 18 (word)
    XOR     EAX, EAX
    MOV     AX, [0x8000 + 18]
    MOV     [ES:DI + BiosInformationBlock.FbWidth], EAX

    ; YResolution at offset 20 (word)
    XOR     EAX, EAX
    MOV     AX, [0x8000 + 20]
    MOV     [ES:DI + BiosInformationBlock.FbHeight], EAX

    ; Calculate fb_pixels_per_scanline
    ; BytesPerScanLine at offset 16 (word) divided by bytes per pixel
    XOR     EDX, EDX
    XOR     EAX, EAX
    MOV     AX, [0x8000 + 16]   ; BytesPerScanLine
    XOR     BX, BX
    MOV     BL, [0x8000 + 25]   ; BitsPerPixel
    SHR     BX, 3               ; bytes per pixel
    DIV     BX
    MOV     [ES:DI + BiosInformationBlock.FbPixelsPerScanline], EAX

    ; Calculate fb_size = height * BytesPerScanLine
    XOR     EDX, EDX
    MOV     AX, [0x8000 + 20]   ; YResolution
    MOV     BX, [0x8000 + 16]   ; BytesPerScanLine
    MUL     BX                  ; DX:AX = height * scanline
    MOV     [ES:DI + BiosInformationBlock.FbSize], AX
    MOV     [ES:DI + BiosInformationBlock.FbSize + 2], DX
    MOV     DWORD [ES:DI + BiosInformationBlock.FbSize + 4], 0

    CLC
    RET

.Error:
    STC
    RET

; Include some helper functions
%INCLUDE "functions.asm"
%INCLUDE "longmode.asm"                                                                                                                               

BIB_OFFSET      EQU 0x1000  ; BIOS Information Block
MEM_OFFSET      EQU 0X1200  ; Memmory Map
Year1           DW 0x00
Year2           DW 0x00
BootMessage:    DB 'Booting KLDR16.BIN...', 0xD, 0xA, 0x0
MenuMessage:    DB 0xD, 0xA, 'KAOS Boot Menu:', 0xD, 0xA, \
                   '1: VGA Text Mode (80x25)', 0xD, 0xA, \
                   '2: VBE Graphics Mode (1024x768)', 0xD, 0xA, \
                   'Select Option: ', 0x0
VbeFailedMessage: DB 0xD, 0xA, 'VBE setup failed! Falling back to Text Mode...', 0xD, 0xA, 0x0