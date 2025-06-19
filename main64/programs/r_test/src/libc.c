#include "libc.h"

// Prints out the root directory of the FAT12 partition
int PrintRootDirectory()
{
    return SYSCALL0(SYSCALL_PRINTROOTDIRECTORY);
}

int ClearScreen()
{
    return SYSCALL0(SYSCALL_CLEARSCREEN);
}

// Prints out a null-terminated string
void printf_syscall_wrapper(unsigned char *string)
{
    SYSCALL1(SYSCALL_PRINTF, string);
}

// Returns the entered character
unsigned char getchar_syscall_wrapper()
{
    long enteredCharacter = SYSCALL0(SYSCALL_GETCHAR);

    return (unsigned char)enteredCharacter;
}

// Reads a string with the given size from the keyboard, and returns it
void scanf_syscall_wrapper(unsigned char *buffer, int buffer_size)
{
    int processKey = 1;
    int i = 0;
    
    while (i < buffer_size - 1)
    {
        unsigned char key = 0;

        while (key == 0)
            key = getchar_syscall_wrapper();
        // key = 'A';

        processKey = 1;
        
        // When we have hit the ENTER key, we have finished entering our input data
        if (key == KEY_RETURN)
        {
            // printf_syscall_wrapper("\n");
            break;
        }
        
        /* if (key == KEY_BACKSPACE)
        {
            processKey = 0;
        
            // We only process the backspace key, if we have data already in the input buffer
            if (i > 0)
            {
                int col;
                int row;
            
                // Move the cursor position one character back
                GetCursorPosition(&row, &col);
                col -= 1;
                SetCursorPosition(&row, &col);
            
                // Clear out the last printed key
                // This also moves the cursor one character forward, so we have to go back
                // again with the cursor in the next step
                printf_syscall_wrapper(" ");
                
                // Move the cursor position one character back again
                GetCursorPosition(&row, &col);
                col -= 1;
                SetCursorPosition(&row, &col);
            
                // Delete the last entered character from the input buffer
                i--;
            }
        } */
        
        if (processKey == 1)
        {
            // Print out the current entered key stroke
            // If we have pressed a non-printable key, the character is not printed out
            if (key != 0)
            {
                char str[2] = {key};
                printf_syscall_wrapper(str);
            }
        
            // Write the entered character into the provided buffer
            buffer[i] = key;
            i++;
        }
    }
    
    // Null-terminate the input string
    buffer[i] = '\0';
}