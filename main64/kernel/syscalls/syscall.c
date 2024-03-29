#include "../multitasking/multitasking.h"
#include "../drivers/screen.h"
#include "../drivers/keyboard.h"
#include "../io/fat12.h"
#include "../common.h"
#include "syscall.h"

// Implements the SysCall Handler
// 
// CAUTION!
// When the function "SysCallHandlerC" is executed, Interrupts are disabled (performed in the
// Assembler code).
// Therefore, it is *safe* to call other functions in the Kernel (like "GetTaskState"), 
// because a Context Switch can't happen, because of the disabled Timer Interrupt.
// But we can't call functions that are causing Page Fault, because of the disabled interrupts
// we can't handle Page Faults.
unsigned long SysCallHandlerC(SysCallRegisters *Registers)
{
    // The SysCall Number is stored in the register RDI
    int sysCallNumber = Registers->RDI;

    // printf
    if (sysCallNumber == SYSCALL_PRINTF)
    {
        printf((char *)Registers->RSI);

        return 1;
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

        return 1;
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

        return 1;
    }
    // SetCursor
    else if (sysCallNumber == SYSCALL_SETCURSOR)
    {
        int *row = (int *)Registers->RSI;
        int *col = (int *)Registers->RDX;
        SetCursorPosition(*row, *col);

        return 1;
    }
    // ExecuteUserProcess
    else if (sysCallNumber == SYSCALL_EXECUTE)
    {
        // We can't start directly here in the SysCall handler the requested User Mode program, because interrupts are
        // currently disabled, and therefore we can't load the new program into memory.
        // Loading the program into memory would generate Page Faults that we can't handle, because of the disabled interrupts.
        // 
        // Therefore, we store the User Mode program that we want to start, at the memory location "USERMODE_PROGRAMM_TO_EXECUTE".
        // The Kernel Mode Task "StartUserModeTask()" continuously checks this memory location if there is a new User Mode program
        // to be started.
        // If yes, the program is started, and the memory location is finally cleared.

        // Find the Root Directory Entry for the given program name
        RootDirectoryEntry *entry = FindRootDirectoryEntry((char *)Registers->RSI);

        if (entry != 0)
        {
            // The given program name was found, so we copy the program name to the memory locattion "USERMODE_PROGRAMM_TO_EXECUTE"
            char *fileName = (char *)USERMODE_PROGRAMM_TO_EXECUTE;
            strcpy(fileName, (char *)Registers->RSI);
            return 1;
        }
        else
            return 0;
    }
    // PrintRootDirectory
    else if (sysCallNumber == SYSCALL_PRINTROOTDIRECTORY)
    {
        PrintRootDirectory();
        
        return 1;
    }
    // ClearScreen
    else if (sysCallNumber == SYSCALL_CLEARSCREEN)
    {
        ClearScreen();

        return 1;
    }
    // DeleteFile
    else if (sysCallNumber == SYSCALL_DELETEFILE)
    {
        unsigned char *fileName = (unsigned char *)Registers->RSI;
        unsigned char *extension = (unsigned char *)Registers->RDX;

        return DeleteFile(fileName, extension);
    }
    // OpenFile
    else if(sysCallNumber == SYSCALL_OPENFILE)
    {
        unsigned char *fileName = (unsigned char *)Registers->RSI;
        unsigned char *extension = (unsigned char *)Registers->RDX;
        char *fileMode = (unsigned char *)Registers->RCX;

        return OpenFile(fileName, extension, fileMode);
    }
    // CloseFile
    else if(sysCallNumber == SYSCALL_CLOSEFILE)
    {
        unsigned long fileHandle = (unsigned long)Registers->RSI;

        return CloseFile(fileHandle);
    }
    // ReadFile
    else if(sysCallNumber == SYSCALL_READFILE)
    {
        unsigned long fileHandle = (unsigned long)Registers->RSI;
        unsigned char *buffer = (unsigned char *)Registers->RDX;
        int length = (int)Registers->RCX;

        return ReadFile(fileHandle, buffer, length);
    }
    // WriteFile
    else if (sysCallNumber == SYSCALL_WRITEFILE)
    {
        unsigned long fileHandle = (unsigned long)Registers->RSI;
        unsigned char *buffer = (unsigned char *)Registers->RDX;
        unsigned long length = (int)Registers->RCX;

        return WriteFile(fileHandle, buffer, length);;
    }
    // EndOfFile
    else if(sysCallNumber == SYSCALL_ENDOFFILE)
    {
        unsigned long fileHandle = (unsigned long)Registers->RSI;

        return EndOfFile(fileHandle);
    }
    // SeekFile
    else if (sysCallNumber == SYSCALL_SEEKFILE)
    {
        unsigned long fileHandle = (unsigned long)Registers->RSI;
        unsigned long fileOffset = (unsigned long)Registers->RDX;

        return SeekFile(fileHandle, fileOffset);
    }

    return 0;
}