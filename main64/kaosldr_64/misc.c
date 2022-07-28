#include "misc.h"

// Define a variable for the screen location information
ScreenLocation screenLocation;

char tbuf[64];
char tbuf_long[64];
char bchars[] = {'0','1','2','3','4','5','6','7','8','9','A','B','C','D','E','F'};

// Reads a single char (8 bytes) from the specified port.
unsigned char inb(unsigned short Port)
{
   unsigned char ret;
   asm volatile("inb %1, %0" : "=a" (ret) : "dN" (Port));
   
   return ret;
}

// Reads a single short (16 bytes) from the specific port.
unsigned short inw(unsigned short Port)
{
   unsigned short ret;
   asm volatile ("inw %1, %0" : "=a" (ret) : "dN" (Port));
   
   return ret;
}

// Writes a single char (8 bytes) to the specified port.
void outb(unsigned short Port, unsigned char Value)
{
    asm volatile ("outb %1, %0" : : "dN" (Port), "a" (Value));
}

// Writes a single short (16 bytes) to the specified port.
void outw(unsigned short Port, unsigned short Value)
{
    asm volatile ("outw %1, %0" : : "dN" (Port), "a" (Value));
}

// Writes a single int (32 bytes) to the specified port.
void outl(unsigned short Port, unsigned int Value)
{
    asm volatile ("outl %1, %0" : : "dN" (Port), "a" (Value));
}

// Initializes the screen.
void InitializeScreen()
{
    screenLocation.Row = 1;
    screenLocation.Col = 1;
    screenLocation.Attributes = COLOR_WHITE;
    ClearScreen();
}

// Clears the screen
void ClearScreen()
{
    char *video_memory = (char *)VIDEO_MEMORY;
    int row, col;

    for (row = 0; row < ROWS; row++)
    {
        for (col = 0; col < COLS; col++)
        {
            int offset = row * COLS * 2 + col * 2;
            video_memory[offset] = 0x20; // Blank
            video_memory[offset + 1] = screenLocation.Attributes;
        }
    }

    // Reset the cursor to the beginning
    screenLocation.Row = 1;
    screenLocation.Col = 1;
    MoveCursor();
}

// Returns the current cursor position
void GetCursorPosition(int *Row, int *Col)
{
    *Row = screenLocation.Row;
    *Col = screenLocation.Col;
}

// Sets the current cursor position
void SetCursorPosition(int Row, int Col)
{
    screenLocation.Row = Row;
    screenLocation.Col = Col;
    MoveCursor();
}

// Moves the screen cursor to the current location on the screen.
void MoveCursor()
{
   // Calculate the linear offset of the cursor
   short cursorLocation = (screenLocation.Row - 1) * COLS + (screenLocation.Col - 1);

   // Setting the cursor's high byte
   outb(0x3D4, 14);
   outb(0x3D5, cursorLocation >> 8);
   
   // Setting the cursor's low byte
   outb(0x3D4, 15);
   outb(0x3D5, cursorLocation);
}

// Prints a single character on the screen.
void print_char(char character)
{
    char* video_memory = (char *)VIDEO_MEMORY;
    
    switch(character)
    {
        case '\n':
        {
            // New line
            screenLocation.Row++;
            screenLocation.Col = 1;

            break;
        }
        case '\t':
        {
            // Tab
            screenLocation.Col = (screenLocation.Col + 8) & ~ (8 - 1);
            break;
        }
        default:
        {
            int offset = (screenLocation.Row - 1) * COLS * 2 + (screenLocation.Col - 1) * 2;
            video_memory[offset] = character;
            video_memory[offset + 1] = screenLocation.Attributes;
            screenLocation.Col++;

            break;
        }
    }

    // Scroll();
    MoveCursor();
}

// Prints out a null-terminated string.
void printf(char *string)
{
    while (*string != '\0')
    {
        print_char(*string);
        string++;
    }
}

// Prints out an integer value for a specific base (base 10 => decimal, base 16 => hex).
void printf_int(int i, int base)
{
    char str[32] = "";
    itoa(i, base, str);
    printf(str);
}

// Converts an integer value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void itoa(int i, unsigned base, char *buf)
{
    if (base > 16) return;
    
    if (i < 0)
    {
        *buf++ = '-';
        i *= -1;
    }
    
    itoa_helper(i, base, buf);
}

// Helper function for the itoa function.
// The static keyword means that this function is only available within the scope of this object file.
static void itoa_helper(unsigned short i, unsigned base, char *buf)
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

// A simple strcmp implementation
int strcmp(char *s1, char *s2, int len)
{
    int i = 0;

    while (*s1 && (*s1 == *s2) && i < len)
    {
        s1++;
        s2++;
        i++;
    }

    return *(unsigned char *)s1 - *(unsigned char *)s2;
}