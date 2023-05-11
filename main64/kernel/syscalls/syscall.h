#ifndef SYSCALL_H
#define SYSCALL_H

// Defines the various available SysCalls
#define SYSCALL_PRINTF  1

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