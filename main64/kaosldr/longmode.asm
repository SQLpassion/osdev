%define PAGE_PRESENT    (1 << 0)
%define PAGE_WRITE      (1 << 1)
 
%define CODE_SEG     0x0008
%define DATA_SEG     0x0010
 
ALIGN 4
IDT:
    .Length       dw 0
    .Base         dd 0
 
; Function to switch directly to long mode from real mode.
; Identity maps the first 2MiB.
; Uses Intel syntax.
 
; es:edi    Should point to a valid page-aligned 16KiB buffer, for the PML4, PDPT, PD and a PT.
; ss:esp    Should point to memory that can be used as a small (1 uint32_t) stack
 

; ========================================= 
; Memory Layout of the various Page Tables
; =========================================
; [Page Map Level 4 at 0x9000]
; Entry 001: 0x10000 (es:di + 0x1000)
; Entry ...
; Entry 512

; [Page Directory Pointer Table at 0x10000]
; Entry 001: 0x11000 (es:di + 0x2000)
; Entry ...
; Entry 512

; [Page Directory Table at 0x11000]
; Entry 001: 0x12000 (es:di + 0x3000)
; Entry ...
; Entry 512

; [Page Table at 0x12000]
; Entry 001: 0x000000
; Entry ...
; Entry 512: 0x1FF000

SwitchToLongMode:
    mov edi, 0x9000
    
    ; Zero out the 16KiB buffer.
    ; Since we are doing a rep stosd, count should be bytes/4.   
    push di                           ; REP STOSD alters DI.
    mov ecx, 0x1000
    xor eax, eax
    cld
    rep stosd
    pop di                            ; Get DI back.
 
    ; Build the Page Map Level 4 (PML4)
    ; es:di points to the Page Map Level 4 table.
    lea eax, [es:di + 0x1000]               ; Put the address of the Page Directory Pointer Table in to EAX.
    or eax, PAGE_PRESENT | PAGE_WRITE       ; Or EAX with the flags - present flag, writable flag.
    mov [es:di], eax                        ; Store the value of EAX as the first PML4E.
 
    ; Build the Page Directory Pointer Table (PDP)
    lea eax, [es:di + 0x2000]               ; Put the address of the Page Directory in to EAX.
    or eax, PAGE_PRESENT | PAGE_WRITE       ; Or EAX with the flags - present flag, writable flag.
    mov [es:di + 0x1000], eax               ; Store the value of EAX as the first PDPTE.
 
    ; Build the Page Directory (PD)
    lea eax, [es:di + 0x3000]               ; Put the address of the Page Table in to EAX.
    or eax, PAGE_PRESENT | PAGE_WRITE       ; Or EAX with the flags - present flag, writeable flag.
    mov [es:di + 0x2000], eax               ; Store to value of EAX as the first PDE.
 
    push di                                 ; Save DI for the time being.
    lea di, [di + 0x3000]                   ; Point DI to the page table.
    mov eax, PAGE_PRESENT | PAGE_WRITE      ; Move the flags into EAX - and point it to 0x0000.
    
    ; Build the Page Table (PT)
.LoopPageTable:
    mov [es:di], eax
    add eax, 0x1000
    add di, 8
    cmp eax, 0x200000                 ; If we did all 2MiB, end.
    jb .LoopPageTable
 
    pop di                            ; Restore DI.

    ; Disable IRQs
    mov al, 0xFF                      ; Out 0xFF to 0xA1 and 0x21 to disable all IRQs.
    out 0xA1, al
    out 0x21, al
 
    nop
    nop
 
    lidt [IDT]                        ; Load a zero length IDT so that any NMI causes a triple fault.
 
    ; Enter long mode.
    mov eax, 10100000b                ; Set the PAE and PGE bit.
    mov cr4, eax
 
    mov edx, edi                      ; Point CR3 at the PML4.
    mov cr3, edx
 
    mov ecx, 0xC0000080               ; Read from the EFER MSR. 
    rdmsr    
 
    or eax, 0x00000100                ; Set the LME bit.
    wrmsr
 
    mov ebx, cr0                      ; Activate long mode -
    or ebx, 0x80000001                 ; - by enabling paging and protection simultaneously.
    mov cr0, ebx                    
 
    lgdt [GDT.Pointer]                ; Load GDT.Pointer defined below.
 
    jmp CODE_SEG:LongMode             ; Load CS with 64 bit segment and flush the instruction cache

CheckCPU:
    ; Check whether CPUID is supported or not.
    pushfd                            ; Get flags in EAX register.
 
    pop eax
    mov ecx, eax  
    xor eax, 0x200000 
    push eax 
    popfd
 
    pushfd 
    pop eax
    xor eax, ecx
    shr eax, 21 
    and eax, 1                        ; Check whether bit 21 is set or not. If EAX now contains 0, CPUID isn't supported.
    push ecx
    popfd 
 
    test eax, eax
    jz NoLongMode
 
    mov eax, 0x80000000   
    cpuid                 
 
    cmp eax, 0x80000001               ; Check whether extended function 0x80000001 is available are not.
    jb NoLongMode                    ; If not, long mode not supported.
 
    mov eax, 0x80000001  
    cpuid                 
    test edx, 1 << 29                 ; Test if the LM-bit, is set or not.
    jz NoLongMode                    ; If not Long mode not supported.
 
    ret
 
NoLongMode:
    ; Print out a character
    xor	bx, bx
    mov	ah, 0x0e
    mov	al, 'E'
    int	0x10
   
    jmp $
 
    ; Global Descriptor Table
GDT:
.Null:
    dq 0x0000000000000000             ; Null Descriptor - should be present.
 
.Code:
    dq 0x00209A0000000000             ; 64-bit code descriptor (exec/read).
    dq 0x0000920000000000             ; 64-bit data descriptor (read/write).
 
ALIGN 4
    dw 0                              ; Padding to make the "address of the GDT" field aligned on a 4-byte boundary
 
.Pointer:
    dw $ - GDT - 1                    ; 16-bit Size (Limit) of GDT.
    dd GDT                            ; 32-bit Base Address of GDT. (CPU will zero extend to 64-bit)
 
[BITS 64]
LongMode:
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Setup the stack
	; mov ebp, 0x70000
	; mov esp, ebp
    mov rax, qword 0x70000
    mov rsp, rax
    mov rbp, rsp
    xor rbp, rbp

    ; Blank out the screen to a blue color.
    ; mov edi, 0xB8000
    ; mov rcx, 500                      ; Since we are clearing uint64_t over here, we put the count as Count/4.
    ; mov rax, 0x1F201F201F201F20       ; Set the value to set the screen to: Blue background, white foreground, blank spaces.
    ; rep stosq                         ; Clear the entire screen. 
 
    ; Display "Hello World!"
    mov edi, 0x00b8000              
 
    mov rax, 0x1F6C1F6C1F651F48    
    mov [edi],rax
 
    mov rax, 0x1F6F1F571F201F6F
    mov [edi + 8], rax
 
    mov rax, 0x1F211F641F6C1F72
    mov [edi + 16], rax

    ; Execute the x64 based KAOSLDR
    jmp 0x3000

    ; jmp $