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