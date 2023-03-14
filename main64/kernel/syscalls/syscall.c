#include "../drivers/screen.h"
#include "syscall.h"

// Implements the SysCall Handler
long SysCallHandlerC(SysCallRegisters *Registers)
{
    // The SysCall Number is stored in the register RDI
    int sysCallNumber = Registers->RDI;

    // printf
    if (sysCallNumber == SYSCALL_PRINTF)
    {
        printf((char *)Registers->RSI);

        return 0;
    }
    else if (sysCallNumber == SYSCALL_ADD)
    {
        return Add(
            (int)Registers->RSI,
            (int)Registers->RDX);
    }
    else if (sysCallNumber == SYSCALL_MUL)
    {
        return Mul(
            (int)Registers->RSI,
            (int)Registers->RDX,
            (int)Registers->RCX);
    }
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

long Add(int a, int b)
{
    return a + b;
}

long Mul(int a, int b, int c)
{
    return a * b * c;
}