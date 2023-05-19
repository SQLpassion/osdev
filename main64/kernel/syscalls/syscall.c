#include "../multitasking/multitasking.h"
#include "../drivers/screen.h"
#include "../drivers/keyboard.h"
#include "../common.h"
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
    // getchar
    else if (sysCallNumber == SYSCALL_GETCHAR)
    {
        char returnValue;

        // Get a pointer to the keyboard buffer
        char *keyboardBuffer = (char *)KEYBOARD_BUFFER;
        
        // Copy the entered character into the variable that is returned
        memcpy(&returnValue, keyboardBuffer, 1);

        // Clear the keyboard buffer
        keyboardBuffer[0] = 0;

        // Return the entered character
        return returnValue;
    }
    // GetCursor
    else if (sysCallNumber == SYSCALL_GETCURSOR)
    {
        int *Row = (int *)Registers->RSI;
        int *Col = (int *)Registers->RDX;
        GetCursorPosition(Row, Col);

        return 0;
    }
    // SetCursor
    else if (sysCallNumber == SYSCALL_SETCURSOR)
    {
        int *row = (int *)Registers->RSI;
        int *col = (int *)Registers->RDX;
        SetCursorPosition(*row, *col);

        return 0;
    }

    return 0;
}