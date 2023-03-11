#include "../drivers/screen.h"
#include "syscall.h"

// Implements the SysCall Handler
int SysCallHandlerC(int SysCallNumber, void *Parameters)
{
    // printf
    if (SysCallNumber == SYSCALL_PRINTF)
    {
        printf(Parameters);
        return 0;
    }
}

int RaiseSysCall(int SysCallNumber, void *Parameters)
{
    return RaiseSysCallAsm(SysCallNumber, Parameters);
}