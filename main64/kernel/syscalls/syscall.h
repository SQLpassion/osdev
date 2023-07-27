#ifndef SYSCALL_H
#define SYSCALL_H

// Defines the various available SysCalls
#define SYSCALL_PRINTF              1
#define SYSCALL_GETPID              2
#define SYSCALL_TERMINATE_PROCESS   3
#define SYSCALL_GETCHAR             4
#define SYSCALL_GETCURSOR           5
#define SYSCALL_SETCURSOR           6
#define SYSCALL_EXECUTE             7
#define SYSCALL_PRINTROOTDIRECTORY  8
#define SYSCALL_CLEARSCREEN         9
#define SYSCALL_OPENFILE            10
#define SYSCALL_READFILE            11
#define SYSCALL_WRITEFILE           12
#define SYSCALL_SEEKFILE            13
#define SYSCALL_ENDOFFILE           14
#define SYSCALL_CLOSEFILE           15
#define SYSCALL_DELETEFILE          16

typedef struct SysCallRegisters
{
    // Parameter values
    unsigned long RDI;
    unsigned long RSI;
    unsigned long RDX;
    unsigned long RCX;
    unsigned long R8;
    unsigned long R9;
} SysCallRegisters;

// Implements the SysCall Handler
unsigned long SysCallHandlerC(SysCallRegisters *Registers);

// The SysCall Handler written in Assembler
extern void SysCallHandlerAsm();

#endif