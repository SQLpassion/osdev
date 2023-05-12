#include "syscall.h"

// Raises a SysCall with no parameters
long SYSCALL0(int SysCallNumber)
{
    return SYSCALLASM0(SysCallNumber);
}

// Raises a SysCall with 1 parameter
long SYSCALL1(int SysCallNumber, void *Parameter1)
{
    return SYSCALLASM1(SysCallNumber, Parameter1);
}

// Raises a Syscall with 2 parameters
long SYSCALL2(int SysCallNumber, void *Parameter1, void *Parameter2)
{
    return SYSCALLASM2(SysCallNumber, Parameter1, Parameter2);
}

// Raises a Syscall with 3 parameters
long SYSCALL3(int SysCallNumber, void *Parameter1, void *Parameter2, void *Parameter3)
{
    return SYSCALLASM3(SysCallNumber, Parameter1, Parameter2, Parameter3);
}