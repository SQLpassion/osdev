#ifndef SYSCALL_H
#define SYSCALL_H

// Defines the various available SysCalls
#define SYSCALL_PRINTF 1

// Implements the SysCall Handler
int SysCallHandlerC(int SysCallNumber, void *Parameters);

int RaiseSysCall(int SysCallNumber, void *Parameters);

// The SysCall Handler written in Assembler
extern void SysCallHandlerAsm();

extern int RaiseSysCallAsm();

#endif