#include "../multitasking/multitasking.h"
#include "../drivers/screen.h"
#include "syscall.h"

// Implements the SysCall Handler
// 
// CAUTION!
// When the function "SysCallHandlerC" is executed, Interrupts are disabled (performed in the
// Assembler code).
// Therefore, it is *safe* to call other functions in the Kernel (like "GetTaskState"), 
// because a Context Switch can't happen, because of the disabled Timer Interrupt.
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
    // GetPID
    else if (sysCallNumber == SYSCALL_GETPID)
    {
        Task *state = (Task *)GetTaskState();
        return state->PID;
    }
    // TerminateProcess
    else if (sysCallNumber == SYSCALL_TERMINATE_PROCESS)
    {
        Task *state = (Task *)GetTaskState();
        TerminateTask(state->PID);

        return 0;
    }

    return 0;
}