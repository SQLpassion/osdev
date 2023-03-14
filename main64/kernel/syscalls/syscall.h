#ifndef SYSCALL_H
#define SYSCALL_H

// Defines the various available SysCalls
#define SYSCALL_PRINTF  1
#define SYSCALL_ADD     2
#define SYSCALL_MUL     3

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

// Raises a SysCall with 1 parameter
long SYSCALL1(int SysCallNumber, void *Parameter1);
extern long SYSCALLASM1();

// Raises a Syscall with 2 parameters
long SYSCALL2(int SysCallNumber, void *Parameter1, void *Parameter2);
extern long SYSCALLASM2();

// Raises a Syscall with 3 parameters
long SYSCALL3(int SysCallNumber, void *Parameter1, void *Parameter2, void *Parameter3);
extern long SYSCALLASM3();

// The SysCall Handler written in Assembler
extern void SysCallHandlerAsm();

long Add(int a, int b);
long Mul(int a, int b, int c);

#endif