; Write Text in Teletype Mode:
; AH = 0E
; AL = ASCII character to write

; Set the TTY mode
MOV AH, 0xE

MOV AL, 'H'
INT 0x10

MOV AL, 'e'
INT 0x10

MOV AL, 'l'
INT 0x10
INT 0x10

MOV AL, 'o'
INT 0x10

MOV AL, ' '
INT 0x10

MOV AL , 'W'
INT 0x10

MOV AL, 'o'
INT 0x10

MOV AL, 'r'
INT 0x10

MOV AL, 'l'
INT 0x10

MOV AL, 'd'
INT 0x10

MOV AL, '!'
INT 0x10

JMP $ ; Jump to current address = infinite loop

; Padding and magic number
TIMES 510 - ($-$$) DB 0
DW 0xaa55