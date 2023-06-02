#include "../multitasking/multitasking.h"
#include "../drivers/screen.h"
#include "../drivers/keyboard.h"
#include "../io/fat12.h"
#include "../common.h"
#include "../isr/idt.h"
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
    // ExecuteUserProcess
    else if (sysCallNumber == SYSCALL_EXECUTE)
    {
        // We can't start directly here in the SysCall Handler the requested User Mode program, because the SysCall is executed
        // in the context of a User mode application - like the Shell.
        // Therefore, when we create a copy of the virtual address space for the new User Mode program, we would create a copy
        // of the virtual address space of the program that issued the SysCall - like the Shell. 
        // But we want to create a copy of the Kernel Mode virtual address space.
        // Therefore, we store the User Mode program that we want to start, at the memory location "USERMODE_PROGRAMM_TO_EXECUTE".
        // The Kernel Mode Task "StartUserModeTask()" continuously checks this memory location if there is a new User Mode program
        // to be started.
        // If yes, the Task "StartUserModeTask()" creates a copy of the Kernel Mode virtual address space, then the program is started,
        // and the memory location is finally cleared.
        
        // Find the Root Directory Entry for the given program name
        /* RootDirectoryEntry *entry = FindRootDirectoryEntry((char *)Registers->RSI);
s
        if (entry != 0)
        {
            // The given program name was found, so we copy the program name to the memory location "USERMODE_PROGRAMM_TO_EXECUTE"
            char *fileName = (char *)USERMODE_PROGRAMM_TO_EXECUTE;
            strcpy(fileName, (char *)Registers->RSI);
            return 1;
        }
        else
            return 0; */

        ExecuteUserModeProgramNew((char *)Registers->RSI, 10, Registers->CR3);
        return 1;
    }

    return 0;
}