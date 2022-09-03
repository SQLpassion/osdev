; =================================================================
; This file implements the functionality to identity map the first
; 2MB of physical memory (physical address == virtual address),
; and to switch the CPU into x64 Long Mode.
;
; It finally jumps to 0x3000 and executes the KLDR64.BIN file.
; =================================================================

%define PAGE_PRESENT    (1 << 0)
%define PAGE_WRITE      (1 << 1)
 
%define CODE_SEG     0x0008
%define DATA_SEG     0x0010
 
ALIGN 4

IDT:
    .Length       dw 0
    .Base         dd 0
 
; ========================================= 
; Memory Layout of the various Page Tables
; =========================================
; [Page Map Level 4 at 0x9000]
; Entry 000: 0x10000 (ES:DI + 0x1000)
; Entry ...
; Entry 511

; [Page Directory Pointer Table at 0x10000]
; Entry 000: 0x11000 (ES:DI + 0x2000)
; Entry ...
; Entry 511

; [Page Directory Table at 0x11000]
; Entry 000: 0x12000 (ES:DI + 0x3000)
; Entry ...
; Entry 511

; [Page Table 1 at 0x12000]
; Entry 000: 0x000000
; Entry ...
; Entry 511: 0x1FF000

; =============================================================================
; The following tables are needed for the Higher Half Mapping of the Kernel
;
; Beginning virtual address:
; 0xFFFF800000000000
; 1111111111111111 100000000  000000000  000000000  000000000  000000000000
; Sign Extension   PML4T      PDPT       PDT        PT         Offset
;                  Entry 256  Entry 0    Entry 0    Entry 0
; =============================================================================
; [Page Directory Pointer Table at 0x14000]
; Entry 000: 0x15000 (ES:DI + 0x6000)
; Entry ...
; Entry 511

; [Page Directory Table at 0x15000]
; Entry 000: 0x16000 (ES:DI + 0x7000)
; Entry ...
; Entry 511

; [Page Table 1 at 0x16000]
; Entry 000: 0x000000
; Entry 001: 0x001000
; Entry 002: 0x002000
; Entry ...
; Entry 511: 0x1FF000

SwitchToLongMode:
    MOV     EDI, 0x9000
    
    ; Zero out the 16KiB buffer.
    ; Since we are doing a rep stosd, count should be bytes/4.   
    PUSH    DI                                  ; REP STOSD alters DI.
    MOV     ECX, 0x1000
    XOR     EAX, EAX
    CLD
    REP     STOSD
    POP     DI                                  ; Get DI back.
 
    ; Build the Page Map Level 4 (PML4)
    ; es:di points to the Page Map Level 4 table.
    LEA     EAX, [ES:DI + 0x1000]               ; Put the address of the Page Directory Pointer Table in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE      ; Or EAX with the flags - present flag, writable flag.
    MOV     [ES:DI], EAX                        ; Store the value of EAX as the first PML4E.

    ; =================================================
    ; Needed for the Higher Half Mapping of the Kernel
    ; =================================================
    ; Add the 256th entry to the PML4...
    LEA     EAX, [ES:DI + 0x5000]
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
    LEA     EAX, [ES:DI + 0x6000]               ; Put the address of the Page Directory in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE      ; Or EAX with the flags - present flag, writable flag.
    MOV     [ES:DI + 0x5000], EAX               ; Store the value of EAX as the first PDPTE.
    ; END =================================================
 
    ; Build the Page Directory (PD Entry 1 for 0 - 2 MB)
    LEA     EAX, [ES:DI + 0x3000]               ; Put the address of the Page Table in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE      ; Or EAX with the flags - present flag, writeable flag.
    MOV     [ES:DI + 0x2000], EAX               ; Store to value of EAX as the first PDE.

    ; =================================================
    ; Needed for the Higher Half Mapping of the Kernel
    ; =================================================
    ; Build the Page Directory (PD Entry 1)
    LEA     EAX, [ES:DI + 0x7000]               ; Put the address of the Page Table in to EAX.
    OR      EAX, PAGE_PRESENT | PAGE_WRITE       ; Or EAX with the flags - present flag, writeable flag.
    MOV     [ES:DI + 0x6000], EAX               ; Store to value of EAX as the first PDE.
    ; END =================================================
 
    PUSH    DI                                  ; Save DI for the time being.
    LEA     DI, [DI + 0x3000]                   ; Point DI to the page table.
    MOV     EAX, PAGE_PRESENT | PAGE_WRITE      ; Move the flags into EAX - and point it to 0x0000.
    
    ; Build the Page Table (PT)
.LoopPageTable:
    MOV     [ES:DI], EAX
    ADD     EAX, 0x1000
    ADD     DI, 8
    CMP     EAX, 0x200000                       ; If we did all 2MiB, end.
    JB      .LoopPageTable

    ; =================================================
    ; Needed for the Higher Half Mapping of the Kernel
    ; =================================================
    POP     DI
    PUSH    DI

    LEA     DI, [DI + 0x7000]                   ; Load the address of the 1st Page Table for the Higher Half Mapping of the Kernel
    MOV     EAX, PAGE_PRESENT | PAGE_WRITE      ; Move the flags into EAX - and point it to 0x0000.
    
    ; Build the Page Table (PT 1)
.LoopPageTableHigherHalf:
    MOV     [ES:DI], EAX
    ADD     EAX, 0x1000
    ADD     DI, 8
    CMP     EAX, 0x200000            ; If we did all 2MiB, end.
    JB      .LoopPageTableHigherHalf
    ; END =================================================
 
    POP     DI                                  ; Restore DI.

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

    ; Setup the stack
    MOV     RAX, QWORD 0xFFFF800000050000
    MOV     RSP, RAX
    MOV     RBP, RSP
    XOR     RBP, RBP

    ; Execute the KLDR64.BIN
    JMP     0x3000