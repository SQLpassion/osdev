; =================================================================
; This file implements the functionality to identity map the first
; 16MB of physical memory (physical address == virtual address),
; and to switch the CPU into x64 Long Mode.
;
; Uses 4KB pages (8 Page Tables x 512 entries = 4096 entries = 16MB).
;
; It finally jumps to 0x3000 and executes the KLDR64.BIN file.
; =================================================================

; ================================================================================
;                     x64 LONG MODE PAGE TABLE LAYOUT
;                       (4KB Pages - 16MB Mapped)
; ================================================================================
;
; CR3 Register --> 0x9000 (Physical Address of PML4)
;
; ================================================================================
; PHYSICAL MEMORY LAYOUT OF PAGE TABLES
; ================================================================================
;
;     0x9000  +------------------+
;             |      PML4        |  Page Map Level 4 (4KB)
;     0xA000  +------------------+
;             |      PDPT        |  Page Directory Pointer Table - Identity (4KB)
;     0xB000  +------------------+
;             |       PD         |  Page Directory - Identity (4KB)
;     0xC000  +------------------+
;             |   PDPT (Higher)  |  Page Directory Pointer Table - Higher Half (4KB)
;     0xD000  +------------------+
;             |    PD (Higher)   |  Page Directory - Higher Half (4KB)
;     0xE000  +------------------+
;             |   PT[0..7]       |  Page Tables for 16MB (8 x 4KB)
;     0x16000 +------------------+
;
;    Total: 13 pages = 52KB (0x9000 - 0x15FFF)
;
; ================================================================================
; PAGE MAP LEVEL 4 (PML4) @ 0x9000
; ================================================================================
; Each entry covers 512GB of virtual address space
;
;   Index   Offset    Value              Description
;  +-------+--------+------------------+------------------------------------------+
;  |   0   | 0x000  | 0xA003           | -> PDPT @ 0xA000 (P=1, W=1)              |
;  |       |        |                  |    Covers VA 0x0000000000000000 - ...    |
;  +-------+--------+------------------+------------------------------------------+
;  |  1-255| 0x008- | 0x0000           | Not Present                              |
;  |       | 0x7F8  |                  |                                          |
;  +-------+--------+------------------+------------------------------------------+
;  | 256   | 0x800  | 0xC003           | -> PDPT @ 0xC000 (P=1, W=1)              |
;  |       |        |                  |    Covers VA 0xFFFF800000000000 - ...    |
;  +-------+--------+------------------+------------------------------------------+
;  |257-511| 0x808- | 0x0000           | Not Present                              |
;  |       | 0xFF8  |                  |                                          |
;  +-------+--------+------------------+------------------------------------------+
;
; ================================================================================
; IDENTITY MAPPING (Virtual == Physical)
; ================================================================================
;
; PDPT @ 0xA000 (covers first 512GB of virtual space)
; Each entry covers 1GB
;
;   Index   Offset    Value              Description
;  +-------+--------+------------------+------------------------------------------+
;  |   0   | 0x000  | 0xB003           | -> PD @ 0xB000 (P=1, W=1)                |
;  +-------+--------+------------------+------------------------------------------+
;  | 1-511 | 0x008- | 0x0000           | Not Present                              |
;  |       | 0xFF8  |                  |                                          |
;  +-------+--------+------------------+------------------------------------------+
;
; PD @ 0xB000 (covers first 1GB)
; Each entry points to a 4KB Page Table (512 entries = 2MB per PT)
;
;   Index   Offset    Value              Description
;  +-------+--------+------------------+------------------------------------------+
;  |   0   | 0x000  | 0xE003           | -> PT0 @ 0xE000 (P=1, W=1)               |
;  |   1   | 0x008  | 0xF003           | -> PT1 @ 0xF000 (P=1, W=1)               |
;  |   2   | 0x010  | 0x10003          | -> PT2 @ 0x10000 (P=1, W=1)              |
;  |   3   | 0x018  | 0x11003          | -> PT3 @ 0x11000 (P=1, W=1)              |
;  |   4   | 0x020  | 0x12003          | -> PT4 @ 0x12000 (P=1, W=1)              |
;  |   5   | 0x028  | 0x13003          | -> PT5 @ 0x13000 (P=1, W=1)              |
;  |   6   | 0x030  | 0x14003          | -> PT6 @ 0x14000 (P=1, W=1)              |
;  |   7   | 0x038  | 0x15003          | -> PT7 @ 0x15000 (P=1, W=1)              |
;  +-------+--------+------------------+------------------------------------------+
;  | 8-511 | 0x040- | 0x0000           | Not Present                              |
;  |       | 0xFF8  |                  |                                          |
;  +-------+--------+------------------+------------------------------------------+
;
; ================================================================================
; HIGHER HALF MAPPING (Virtual 0xFFFF800000000000+ -> Physical 0x0+)
; ================================================================================
;
; PDPT @ 0xC000 (covers VA 0xFFFF800000000000 - 0xFFFF807FFFFFFFFF)
; Each entry covers 1GB
;
;   Index   Offset    Value              Description
;  +-------+--------+------------------+------------------------------------------+
;  |   0   | 0x000  | 0xD003           | -> PD @ 0xD000 (P=1, W=1)                |
;  +-------+--------+------------------+------------------------------------------+
;  | 1-511 | 0x008- | 0x0000           | Not Present                              |
;  |       | 0xFF8  |                  |                                          |
;  +-------+--------+------------------+------------------------------------------+
;
; PD @ 0xD000 (covers first 1GB of higher half)
; Each entry points to the same PTs used for identity mapping
;
;   Index   Offset    Value              Description
;  +-------+--------+------------------+------------------------------------------+
;  |   0   | 0x000  | 0xE003           | -> PT0 @ 0xE000 (P=1, W=1)               |
;  |   1   | 0x008  | 0xF003           | -> PT1 @ 0xF000 (P=1, W=1)               |
;  |   2   | 0x010  | 0x10003          | -> PT2 @ 0x10000 (P=1, W=1)              |
;  |   3   | 0x018  | 0x11003          | -> PT3 @ 0x11000 (P=1, W=1)              |
;  |   4   | 0x020  | 0x12003          | -> PT4 @ 0x12000 (P=1, W=1)              |
;  |   5   | 0x028  | 0x13003          | -> PT5 @ 0x13000 (P=1, W=1)              |
;  |   6   | 0x030  | 0x14003          | -> PT6 @ 0x14000 (P=1, W=1)              |
;  |   7   | 0x038  | 0x15003          | -> PT7 @ 0x15000 (P=1, W=1)              |
;  +-------+--------+------------------+------------------------------------------+
;  | 8-511 | 0x040- | 0x0000           | Not Present                              |
;  |       | 0xFF8  |                  |                                          |
;  +-------+--------+------------------+------------------------------------------+
;
; ================================================================================
; VIRTUAL ADDRESS TRANSLATION EXAMPLE
; ================================================================================
;
; Example: Translate VA 0xFFFF800000100000 (Kernel entry point)
;
;   64-bit Virtual Address: 0xFFFF800000100000
;   Binary breakdown:
;
;   1111111111111111 100000000 000000000 000000000 000000000 000000000000
;   |_______________|    |         |         |         |         |
;    Sign Extension   PML4[256] PDPT[0]    PD[0]    (unused)   Offset
;                                           |
;                                        PT level
;                                     4KB pages
;
;   Step 1: PML4[256] @ 0x9800 -> 0xC003 -> PDPT @ 0xC000
;   Step 2: PDPT[0]   @ 0xC000 -> 0xD003 -> PD @ 0xD000
;   Step 3: PD[0]     @ 0xD000 -> 0xE003 -> PT0 @ 0xE000
;   Step 4: PT[256]   @ 0xE800 -> 0x00100003 -> 4KB page @ Physical 0x100000
;   Step 5: Offset within 4KB page: 0x000
;
;   Result: Physical Address = 0x000000 + 0x100000 = 0x100000
;
; ================================================================================
; SHARED PAGE TABLES - WHY IT WORKS
; ================================================================================
;
; Both the identity mapping (PD @ 0xB000) and higher-half mapping (PD @ 0xD000)
; point to the SAME 8 Page Tables (PT0-PT7 @ 0xE000-0x15FFF).
;
; This works because Page Table entries contain PHYSICAL addresses:
;   - PT[256] contains 0x00100003 (physical addr 0x100000 + flags)
;
; When accessed via identity mapping:
;   VA 0x100000 -> PD[0] -> PT0[256] -> PA 0x100000
;
; When accessed via higher-half mapping:
;   VA 0xFFFF800000100000 -> PD[0] -> PT0[256] -> PA 0x100000
;
; Benefits:
;   - Saves 32KB (8 fewer page tables)
;   - Both mappings always stay in sync
;   - Changes to page permissions apply to both mappings
;
; ================================================================================
; PHYSICAL MEMORY MAP (Runtime Layout)
; ================================================================================
;
;     0x016000  +------------------------+
;               |        ...             |  (Page tables end at 0x15FFF)
;     0x100000  +------------------------+  <-- 1MB Mark
;               |     KERNEL.BIN         |  Rust Kernel loaded here
;               |        |               |
;               |        v               |  Kernel code + data + BSS
;               |       ...              |
;               +- - - - - - - - - - - - +  <-- __bss_end (page-aligned)
;               |   PMM Layout Header    |  PmmLayoutHeader struct (8 bytes)
;               |   PMM Region Array     |  PmmRegion[] (40 bytes each)
;               |   PMM Bitmaps          |  Allocation bitmaps (variable size)
;               +- - - - - - - - - - - - +
;               |                        |
;               |  Reserved for Stack    |  All pages marked USED in PMM
;               |        ^               |  Stack grows downward
;               |      Stack             |
;     0x400000  +------------------------+  <-- 4MB Mark / Stack Top (RSP)
;               |        ...             |
;               |   Free Memory          |  First allocatable pages
;               |        ...             |
;     0x1000000 +------------------------+  <-- 16MB (End of mapped region)
;
; ================================================================================

%define PAGE_PRESENT    (1 << 0)
%define PAGE_WRITE      (1 << 1)
 
%define CODE_SEG     0x0008
%define DATA_SEG     0x0010
 
ALIGN 4

IDT:
    .Length       dw 0
    .Base         dd 0


SwitchToLongMode:
    MOV     EDI, 0x9000
    
    ; Zero out the 52KiB buffer (page tables).
    ; Since we are doing a rep stosd, count should be bytes/4.
    MOV     EDI, 0x9000
    MOV     ECX, 0x3400
    XOR     EAX, EAX
    CLD
    A32     REP STOSD
    MOV     EDI, 0x9000
 
    ; Build the Page Map Level 4 (PML4)
    ; es:di points to the Page Map Level 4 table.
    LEA     EAX, [ES:DI + 0x1000]               ; Put the address of the Page Directory Pointer Table in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE      ; Or EAX with the flags - present flag, writable flag.
    MOV     [ES:DI], EAX                        ; Store the value of EAX as the first PML4E.

    ; =================================================
    ; Needed for the Higher Half Mapping of the Kernel
    ; =================================================
    ; Add the 256th entry to the PML4...
    LEA     EAX, [ES:DI + 0x3000]
    OR      EAX, PAGE_PRESENT | PAGE_WRITE
    MOV     [ES:DI + 0x800], EAX                ; 256th entry * 8 bytes per entry
    ; END =================================================
    
    ; Build the Page Directory Pointer Table (PDP)
    LEA     EAX, [ES:DI + 0x2000]               ; Put the address of the Page Directory in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE      ; Or EAX with the flags - present flag, writable flag.
    MOV     [ES:DI + 0x1000], EAX               ; Store the value of EAX as the first PDPTE.

    ; =================================================
    ; Needed for the Higher Half Mapping of the Kernel
    ; =================================================
    ; Build the Page Directory Pointer Table (PDP)
    LEA     EAX, [ES:DI + 0x4000]               ; Put the address of the Page Directory in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE      ; Or EAX with the flags - present flag, writable flag.
    MOV     [ES:DI + 0x3000], EAX               ; Store the value of EAX as the first PDPTE.
    ; END =================================================
 
    ; Build the Page Directory using 4KB page tables.
    ; Each PDE points to one PT (512 entries = 2MB per PT).
    PUSH    DI
    LEA     DI, [DI + 0x2000]                   ; Point to Page Directory for identity mapping
    MOV     EAX, 0xE000
    OR      EAX, PAGE_PRESENT | PAGE_WRITE
    MOV     ECX, 8                              ; 8 PTs = 16MB
.LoopPDE:
    MOV     [ES:DI], EAX
    ADD     EAX, 0x1000                         ; Next PT page
    ADD     DI, 8                               ; Next PDE entry (8 bytes)
    DEC     ECX
    JNZ     .LoopPDE
    POP     DI

    ; =================================================
    ; Needed for the Higher Half Mapping of the Kernel
    ; =================================================
    ; Build the Page Directory for higher half using the same PTs.
    ; Maps virtual 0xFFFF800000000000+ to physical 0 - 16MB.
    PUSH    DI
    LEA     DI, [DI + 0x4000]                   ; Point to Higher Half Page Directory
    MOV     EAX, 0xE000
    OR      EAX, PAGE_PRESENT | PAGE_WRITE
    MOV     ECX, 8                              ; 8 PTs = 16MB
.LoopPDEHigherHalf:
    MOV     [ES:DI], EAX
    ADD     EAX, 0x1000                         ; Next PT page
    ADD     DI, 8                               ; Next PDE entry
    DEC     ECX
    JNZ     .LoopPDEHigherHalf
    POP     DI
    ; END =================================================

    ; Build the Page Tables (4KB pages).
    ; Maps physical 0 - 16MB into the 8 PTs.
    PUSH    EDI
    LEA     EDI, [EDI + 0x5000]                 ; Point to PT0 @ 0xE000
    XOR     EBX, EBX                            ; Physical address counter
    MOV     ECX, 4096                           ; 16MB / 4KB
.LoopPTE:
    MOV     EAX, EBX
    OR      EAX, PAGE_PRESENT | PAGE_WRITE
    MOV     [ES:EDI], EAX
    ADD     EBX, 0x1000
    ADD     EDI, 8
    DEC     ECX
    JNZ     .LoopPTE
    POP     EDI

    ; Disable IRQs
    MOV     AL, 0xFF                            ; Out 0xFF to 0xA1 and 0x21 to disable all IRQs.
    OUT     0xA1, AL
    OUT     0x21, AL
 
    NOP
    NOP
 
    LIDT    [IDT]                               ; Load a zero length IDT so that any NMI causes a triple fault.
 
    ; Enter long mode.
    MOV     EAX, 10100000b                      ; Set the PAE and PGE bit.
    MOV     CR4, EAX
    MOV     EDX, EDI                            ; Point CR3 at the PML4.
    MOV     CR3, EDX
    MOV     ECX, 0xC0000080                     ; Read from the EFER MSR. 
    RDMSR    
 
    OR      EAX, 0x00000100                     ; Set the LME bit.
    WRMSR
 
    MOV     EBX, CR0                            ; Activate long mode -
    OR      EBX, 0x80000001                     ; - by enabling paging and protection simultaneously.
    MOV     CR0, EBX                    
 
    LGDT    [GDT.Pointer]                       ; Load GDT.Pointer defined below.
 
    JMP     CODE_SEG:LongMode                   ; Load CS with 64 bit segment and flush the instruction cache

CheckCPU:
    ; Check whether CPUID is supported or not.
    PUSHFD                                      ; Get flags in EAX register.
 
    POP     EAX
    MOV     ECX, EAX  
    XOR     EAX, 0x200000 
    PUSH    EAX 
    POPFD
 
    PUSHFD 
    POP     EAX
    XOR     EAX, ECX
    SHR     EAX, 21 
    AND     EAX, 1                              ; Check whether bit 21 is set or not. If EAX now contains 0, CPUID isn't supported.
    PUSH    ECX
    POPFD 
 
    TEST    EAX, EAX
    JZ      .NoLongMode
 
    MOV     EAX, 0x80000000   
    CPUID                 
 
    CMP     EAX, 0x80000001                     ; Check whether extended function 0x80000001 is available are not.
    JB      .NoLongMode                          ; If not, long mode not supported.
 
    MOV     EAX, 0x80000001  
    CPUID                 
    TEST    EDX, 1 << 29                        ; Test if the LM-bit, is set or not.
    JZ      .NoLongMode                          ; If not Long mode not supported.
 
    ret
 
.NoLongMode:
    ; Print out a character
    XOR	    BX, BX
    MOV	    AH, 0x0E
    MOV	    AL, 'E'
    INT	    0x10
   
    JMP     $
 
    ; Global Descriptor Table
GDT:
.Null:
    DQ 0x0000000000000000                   ; Null Descriptor - should be present.
 
.Code:
    DQ 0x00209A0000000000                   ; 64-bit code descriptor (exec/read).
    DQ 0x0000920000000000                   ; 64-bit data descriptor (read/write).
 
ALIGN 4
    DW 0                                    ; Padding to make the "address of the GDT" field aligned on a 4-byte boundary
 
.Pointer:
    DW $ - GDT - 1                          ; 16-bit Size (Limit) of GDT.
    DD GDT                                  ; 32-bit Base Address of GDT. (CPU will zero extend to 64-bit)
 
[BITS 64]
LongMode:
    MOV     AX, DATA_SEG
    MOV     DS, AX
    MOV     ES, AX
    MOV     FS, AX
    MOV     GS, AX
    MOV     SS, AX

    ; Setup the stack at physical address 0x400000 (4MB).
    ; Stack grows downward, giving ~2MB of stack space before hitting kernel area.
    ; With 16MB mapped, this is safely within the identity-mapped region.
    ; Note: Can't use higher-half address on Apple Silicon with UTM.
    MOV     RAX, QWORD 0x400000

    ; The remaining part works as expected
    MOV     RSP, RAX
    MOV     RBP, RSP
    XOR     RBP, RBP

    ; Execute the KLDR64.BIN
    JMP     0x3000
