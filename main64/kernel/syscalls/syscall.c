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
}