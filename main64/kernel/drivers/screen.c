#include "screen.h"
#include "../common.h"

// Define a variable for the screen location information
ScreenLocation screenLocation;

// Initializes the screen
void InitializeScreen()
{
    screenLocation.Row = 1;
    screenLocation.Col = 1;
    screenLocation.Attributes = COLOR_WHITE;
    ClearScreen();
}

// Sets the specific color
int SetColor(int Color)
{
    int currentColor = screenLocation.Attributes;
    screenLocation.Attributes = Color;

    return currentColor;
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

// Moves the screen cursor to the current location on the screen
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

// Scrolls the screen, when we have used more than 25 rows
void Scroll()
{
   // Get a space character with the default colour attributes.
   unsigned char attributeByte = (0 /*black*/ << 4) | (15 /*white*/ & 0x0F);
   unsigned short blank = 0x20 /* space */ | (attributeByte << 8);
   char* video_memory = (char *)VIDEO_MEMORY;

   // Row 25 is the end, this means we need to scroll up
   if (screenLocation.Row > ROWS)
   {
       int i;
       for (i = 0; i < COLS * 2 * (ROWS - 1); i++)
       {
           video_memory[i] = video_memory[i + (COLS * 2)];
       }

       // Blank the last line
       for (i = (ROWS - 1) * COLS * 2; i < ROWS * COLS * 2; i += 2)
       {
           video_memory[i] = blank;
           video_memory[i + 1] = attributeByte;
       }

       screenLocation.Row = 25;
   }
}

// Prints out a null-terminated string
void printf(char *string)
{
    while (*string != '\0')
    {
        print_char(*string);
        string++;
    }
}

// Prints a single character on the screen
void print_char(char character)
{
    char* video_memory = (char *)VIDEO_MEMORY;
    
    switch(character)
    {
        case CRLF:
        {
            // New line
            screenLocation.Row++;
            screenLocation.Col = 1;

            break;
        }
        case TAB:
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

    Scroll();
    MoveCursor();
}

// Prints out an integer value
void printf_int(int i, int base)
{
    char str[32] = "";
    itoa(i, base, str);
    printf(str);
}

// Prints out a long value
void printf_long(long i, int base)
{
    char str[32] = "";
    ltoa(i, base, str);
    printf(str);
}