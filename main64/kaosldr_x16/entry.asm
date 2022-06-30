[ORG 0x1200]
[BITS 16]

MAIN:
    ; Print out a welcome message
    MOV     SI, BootMessage
    CALL    PrintString

    ; Get the current date from the BIOS
    CALL    GetDate

    ; Get the current time from the BIOS
    CALL    GetTime

    ; Print out the current date - year part
    MOV     SI, YearString
    CALL    PrintString
    MOV     AX, [Year]
    CALL    PrintDecimal
    MOV     SI, CRLF
    CALL    PrintString

    ; Print out the current date - month part
    MOV     SI, MonthString
    CALL    PrintString
    MOV     AX, [Month]
    CALL    PrintDecimal
    MOV     SI, CRLF
    CALL    PrintString

    ; Print out the current date - day part
    MOV     SI, DayString
    CALL    PrintString
    MOV     AX, [Day]
    CALL    PrintDecimal
    MOV     SI, CRLF
    CALL    PrintString

    ; Print out the current time - hour part
    MOV     SI, HourString
    CALL    PrintString
    MOV     AX, [Hour]
    CALL    PrintDecimal
    MOV     SI, CRLF
    CALL    PrintString

    ; Print out the current time - minute part
    MOV     SI, MinuteString
    CALL    PrintString
    MOV     AX, [Minute]
    CALL    PrintDecimal
    MOV     SI, CRLF
    CALL    PrintString

    ; Print out the current time - second part
    MOV     SI, SecondString
    CALL    PrintString
    MOV     AX, [Second]
    CALL    PrintDecimal
    MOV     SI, CRLF
    CALL    PrintString

    RET

; Include some helper functions
%INCLUDE "functions.asm"                                                                                                                                        

BootMessage:    DB 'Executing KAOSLDR_x16.bin...', 0xD, 0xA, 0x0
YearString:     DB 'Year: ', 0x0
MonthString:    DB 'Month: ', 0x0
DayString:      DB 'Day: ', 0x0
HourString:     DB 'Hour: ', 0x0
MinuteString:   DB 'Minute: ', 0x0
SecondString:   DB 'Second: ', 0x0
CRLF:           DB 0xD, 0xA, 0x0

Year1           DW 0x00
Year2           DW 0x00
Year            DW 0x00
Month           DW 0x00
Day             DW 0x00

Hour            DW 0x00
Minute          DW 0x00
Second          DW 0x00