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
#define SYSCALL_CREATEFILE          10
#define SYSCALL_DELETEFILE          11
#define SYSCALL_OPENFILE            12
#define SYSCALL_CLOSEFILE           13
#define SYSCALL_READFILE            14
#define SYSCALL_WRITEFILE           15
#define SYSCALL_ENDOFFILE           16
#define SYSCALL_SEEKFILE            17

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
long SysCallHandlerC(SysCallRegisters *Registers);

// The SysCall Handler written in Assembler
extern void SysCallHandlerAsm();

#endif