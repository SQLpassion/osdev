;************************************************;
; This file contains some helper functions.
;************************************************;

; This structure stores all the information that we retrieve from the BIOS while we are in x16 Real Mode
STRUC BiosInformationBlock
    .Year:      RESD 1
    .Month:     RESW 1
    .Day:       RESW 1
    .Hour:      RESW 1
    .Minute:    RESW 1
    .Second:    RESW 1
ENDSTRUC

struc	MemoryMapEntry
	.baseAddress	resq	1	; base address of address range
	.length		    resq	1	; length of address range in bytes
	.type		    resd	1	; type of address range
	.acpi_null	    resd	1	; reserved
endstruc

;================================================
; This function prints a whole string, where the 
; input string is stored in the register "SI"
;================================================
PrintString:
    ; Set the TTY mode
    MOV     AH, 0xE
    INT     10

    ; Set the input string
    MOV     AL, [SI]
    CMP     AL, 0
    JE      PrintString_End
    
    INT     0x10
    INC     SI
    JMP     PrintString
    
    PrintString_End:
RET

;================================================
; This function prints out a decimal number
; that is stored in the register AX.
;================================================
PrintDecimal:
    MOV     CX, 0
    MOV     DX, 0

PrintDecimal_Start:
    CMP     AX ,0
    JE      PrintDecimal_Print
    MOV     BX, 10
    DIV     BX
    PUSH    DX
    INC     CX
    XOR     DX, DX
    JMP     PrintDecimal_Start
PrintDecimal_Print:
    CMP     CX, 0
    JE      PrintDecimal_Exit
    POP     DX
        
    ; Add 48 so that it represents the ASCII value of digits
    MOV     AL, DL
    ADD     AL, 48
    MOV     AH, 0xE
    INT     0x10

    DEC     CX
    JMP     PrintDecimal_Print
PrintDecimal_Exit:
RET

;================================================
; This function converts a BCD number to a
; decimal number.
;================================================
Bcd2Decimal:
    MOV     CL, AL
    SHR     AL, 4
    MOV     CH, 10
    MUL     CH
    AND     CL, 0Fh
    ADD     AL, CL
RET

;=================================================
; This function retrieves the date from the BIOS.
;=================================================
GetDate:
    ; Get the current date from the BIOS
    MOV     AH, 0x4
    INT     0x1A

    ; Century
    PUSH    CX
    MOV     AL, CH
    CALL    Bcd2Decimal
    MOV     [Year1], AX
    POP     CX

    ; Year
    MOV     AL, CL
    CALL    Bcd2Decimal
    MOV     [Year2], AX

    ; Month
    MOV     AL, DH
    CALL    Bcd2Decimal 
    MOV     WORD [ES:DI + BiosInformationBlock.Month], AX

    ; Day
    MOV     AL, DL
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Day], AX

    ; Calculate the whole year (e.g. "20" * 100 + "22" = 2022)
    MOV     AX, [Year1]
    MOV     BX, 100
    MUL     BX
    MOV     BX, [Year2]
    ADD     AX, BX
    MOV     WORD [ES:DI + BiosInformationBlock.Year], AX
RET

;=================================================
; This function retrieves the time from the BIOS.
;=================================================
GetTime:
    ; Get the current time from the BIOS
    MOV     AH, 0x2
    INT     0x1A

    ; Hour
    PUSH    CX
    MOV     AL, CH
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Hour], AX
    POP     CX

    ; Minute
    MOV     AL, CL
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Minute], AX

    ; Second
    MOV     AL, DH
    CALL    Bcd2Decimal
    MOV     WORD [ES:DI + BiosInformationBlock.Second], AX
RET

;=============================================
; This function enables the A20 gate
;=============================================
EnableA20:
    CLI	                ; Disables interrupts
    PUSH	AX          ; Save AX on the stack
    MOV     AL, 2
    OUT     0x92, AL
    POP	    AX          ; Restore the value of AX from the stack
    STI                 ; Enable the interrupts again
RET 

;---------------------------------------------
;	Get memory map from bios
;	/in es:di->destination buffer for entries
;	/ret bp=entry count
;---------------------------------------------
GetMemoryMap:
	pushad
	xor	ebx, ebx
	xor	bp, bp			; number of entries stored here
	mov	edx, 'PAMS'		; 'SMAP'
	mov	eax, 0xe820
	mov	ecx, 24			; memory map entry struct is 24 bytes
	int	0x15			; get first entry
	jc	.error	
	cmp	eax, 'PAMS'		; bios returns SMAP in eax
	jne	.error
	test	ebx, ebx		; if ebx=0 then list is one entry long; bail out
	je	.error
	jmp	.start
.next_entry:
	mov	edx, 'PAMS'		; some bios's trash this register
	mov	ecx, 24			; entry is 24 bytes
	mov	eax, 0xe820
	int	0x15			; get next entry
.start:
	jcxz	.skip_entry		; if actual returned bytes is 0, skip entry
.notext:
	mov	ecx, [es:di + MemoryMapEntry.length]	; get length (low dword)
	test	ecx, ecx		; if length is 0 skip it
	jne	short .good_entry
	mov	ecx, [es:di + MemoryMapEntry.length + 4]; get length (upper dword)
	jecxz	.skip_entry		; if length is 0 skip it
.good_entry:
    inc	bp			    ; increment entry count
	add	di, 24			; point di to next entry in buffer
.skip_entry:
	cmp	ebx, 0			; if ebx return is 0, list is done
	jne	.next_entry		; get next entry
	jmp	.done
.error:
    xor	bx, bx
	mov	ah, 0x0e
	mov	al, 'E'
	int	0x10

	stc
.done:
	popad
	ret


mmap_ent equ 0x8000             ; the number of entries will be stored at 0x8000
do_e820:
    mov di, 0x8004          ; Set di to 0x8004. Otherwise this code will get stuck in `int 0x15` after some entries are fetched 
	xor ebx, ebx		; ebx must be 0 to start
	xor bp, bp		; keep an entry count in bp
	mov edx, 0x0534D4150	; Place "SMAP" into edx
	mov eax, 0xe820
	mov [es:di + 20], dword 1	; force a valid ACPI 3.X entry
	mov ecx, 24		; ask for 24 bytes
	int 0x15
	jc short .failed	; carry set on first call means "unsupported function"
	mov edx, 0x0534D4150	; Some BIOSes apparently trash this register?
	cmp eax, edx		; on success, eax must have been reset to "SMAP"
	jne short .failed
	test ebx, ebx		; ebx = 0 implies list is only 1 entry long (worthless)
	je short .failed
	jmp short .jmpin
.e820lp:
	mov eax, 0xe820		; eax, ecx get trashed on every int 0x15 call
	mov [es:di + 20], dword 1	; force a valid ACPI 3.X entry
	mov ecx, 24		; ask for 24 bytes again
	int 0x15
	jc short .e820f		; carry set means "end of list already reached"
	mov edx, 0x0534D4150	; repair potentially trashed register
.jmpin:
	jcxz .skipent		; skip any 0 length entries
	cmp cl, 20		; got a 24 byte ACPI 3.X response?
	jbe short .notext
	test byte [es:di + 20], 1	; if so: is the "ignore this data" bit clear?
	je short .skipent
.notext:
	mov ecx, [es:di + 8]	; get lower uint32_t of memory region length
	or ecx, [es:di + 12]	; "or" it with upper uint32_t to test for zero
	jz .skipent		; if length uint64_t is 0, skip entry
	inc bp			; got a good entry: ++count, move to next storage spot
	add di, 24
.skipent:
	test ebx, ebx		; if ebx resets to 0, list is complete
	jne short .e820lp
.e820f:
	mov [mmap_ent], bp	; store the entry count
	clc			; there is "jc" on end of list to this point, so the carry must be cleared
	ret
.failed:
	stc			; "function unsupported" error exit
	ret