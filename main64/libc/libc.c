#include "syscall.h"
#include "libc.h"

char tbuf[64];
char tbuf_long[64];
char bchars[] = {'0','1','2','3','4','5','6','7','8','9','A','B','C','D','E','F'};

// Prints out a null-terminated string
void printf(char *string)
{
    SYSCALL1(SYSCALL_PRINTF, string);
}

// Returns the PID of the current executing process
long GetPID()
{
    return SYSCALL0(SYSCALL_GETPID);
}

// Terminates the current executing process
void TerminateProcess()
{
    SYSCALL0(SYSCALL_TERMINATE_PROCESS);
    while (1 == 1) {}
}

// Returns the entered character
char getchar()
{
    long enteredCharacter = SYSCALL0(SYSCALL_GETCHAR);

    return (char)enteredCharacter;
}

// Returns the current cursor position
void GetCursorPosition(int *Row, int *Col)
{
    SYSCALL2(SYSCALL_GETCURSOR, Row, Col);
}

// Sets the current cursor position
void SetCursorPosition(int *Row, int *Col)
{
    SYSCALL2(SYSCALL_SETCURSOR, Row, Col);
}

// Reads a string with the given size from the keyboard, and returns it
void scanf(char *buffer, int buffer_size)
{
    int processKey = 1;
    int i = 0;
    
    while (i < buffer_size)
    {
        char key = 0;

        while (key == 0)
            key = getchar();

        processKey = 1;
        
        // When we have hit the ENTER key, we have finished entering our input data
        if (key == KEY_RETURN)
        {
            printf("\n");
            break;
        }
        
        if (key == KEY_BACKSPACE)
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
                printf(" ");
                
                // Move the cursor position one character back again
                GetCursorPosition(&row, &col);
                col -= 1;
                SetCursorPosition(&row, &col);
            
                // Delete the last entered character from the input buffer
                i--;
            }
        }
        
        if (processKey == 1)
        {
            // Print out the current entered key stroke
            // If we have pressed a non-printable key, the character is not printed out
            if (key != 0)
            {
                char str[2] = {key};
                printf(str);
            }
        
            // Write the entered character into the provided buffer
            buffer[i] = key;
            i++;
        }
    }
    
    // Null-terminate the input string
    buffer[i] = '\0';
}

// Prints out an integer value
void printf_int(int i, int base)
{
    char str[32] = "";
    itoa(i, base, str);
    printf(str);
}

// Prints out a long value
void printf_long(unsigned long i, int base)
{
    char str[32] = "";
    ltoa(i, base, str);
    printf(str);
}

// Converts an integer value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void itoa(unsigned int i, unsigned base, char *buf)
{
    if (base > 16) return;
    
    if (i < 0)
    {
        *buf++ = '-';
        i *= -1;
    }
    
    itoa_helper(i, base, buf);
}

// Converts a long value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void ltoa(unsigned long i, unsigned base, char *buf)
{
    if (base > 16) return;
    
    if (i < 0)
    {
        *buf++ = '-';
        i *= -1;
    }
    
    ltoa_helper(i, base, buf);
}

// Helper function for the itoa function.
void itoa_helper(unsigned int i, unsigned base, char *buf)
{
    int pos = 0;
    int opos = 0;
    int top = 0;
    
    if (i == 0 || base > 16)
    {
        buf[0] = '0';
        buf[1] = '\0';
        return;
    }
    
    while (i != 0)
    {
        tbuf[pos] = bchars[i % base];
        pos++;
        i /= base;
    }
    
    top = pos--;
    
    for (opos = 0; opos < top; pos--,opos++)
    {
        buf[opos] = tbuf[pos];
    }
    
    buf[opos] = 0;
}

// Helper function for the ltoa function.
static void ltoa_helper(unsigned long i, unsigned base, char *buf)
{
    int pos = 0;
    int opos = 0;
    int top = 0;
    
    if (i == 0 || base > 16)
    {
        buf[0] = '0';
        buf[1] = '\0';
        return;
    }
    
    while (i != 0)
    {
        tbuf[pos] = bchars[i % base];
        pos++;
        i /= base;
    }
    
    top = pos--;
    
    for (opos = 0; opos < top; pos--,opos++)
    {
        buf[opos] = tbuf[pos];
    }
    
    buf[opos] = 0;
}

// Checks if a string starts with a given prefix
int StartsWith(char *string, char *prefix)
{
    while (*prefix)
    {
        if (*prefix++ != *string++)
            return 0;
    }

    return 1;
}

int ExecuteUserModeProgram(unsigned char *FileName)
{
    return SYSCALL1(SYSCALL_EXECUTE, FileName);
}