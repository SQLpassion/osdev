; Tell the Assembler that are loaded at offset 0x7c00
[ORG 0x7c00]
[BITS 16]

; Setup the DS and ES register
XOR AX, AX
MOV DS, AX
MOV ES, AX

; Prepare the stack
; Otherwise we can't call a function...
MOV AX, 0x7000
MOV SS, AX
MOV BP, 0x8000
MOV SP, BP

; Print out the 1st string
MOV SI, WelcomeMessage1
CALL PrintLine

; Print out the 2nd string
MOV SI, WelcomeMessage2
CALL PrintLine

JMP $ ; Jump to current address = infinite loop

;================================================
; This function prints a whole string, where the 
; input string is stored in the register "SI"
;================================================
PrintLine:
    ; Set the TTY mode
    MOV AH, 0xE
    INT 10

	MOV AL, [SI]
	CMP AL, 0
	JE End_PrintLine

	INT 0x10
	INC SI
	JMP PrintLine

	End_PrintLine:
RET

; OxA: new line
; 0xD: carriage return
; 0x0: null terminated string
WelcomeMessage1: DB 'Hello World from a barebone bootsector!', 0xD, 0xA, 0x0
WelcomeMessage2: DB 'I hope you enjoy this tutorial...', 0xD, 0xA, 0x0

; Padding and magic number
TIMES 510 - ($-$$) DB 0
DW 0xaa55