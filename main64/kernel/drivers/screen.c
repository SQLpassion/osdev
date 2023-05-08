#include "screen.h"
#include "../common.h"

// Define a variable for the screen location information
ScreenLocation screenLocation;

// The number of rows of the video memory
int NumberOfRows;

// The number of columns of the video memory
int NumberOfColumns;

// The blank character
unsigned char BLANK = 0x20;

// Initializes the screen
void InitializeScreen(int Cols, int Rows)
{
    NumberOfColumns = Cols;
    NumberOfRows = Rows;

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
   short cursorLocation = (screenLocation.Row - 1) * NumberOfColumns + (screenLocation.Col - 1);

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

    for (row = 0; row < NumberOfRows; row++)
    {
        for (col = 0; col < NumberOfColumns; col++)
        {
            int offset = row * NumberOfColumns * 2 + col * 2;
            video_memory[offset] = BLANK;
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
   unsigned char attributeByte = (COLOR_BLACK << 4) | (COLOR_WHITE & 0x0F);
   char *video_memory = (char *)VIDEO_MEMORY;
   int i;

   // Check if we have reached the last row of the screen.
   // This means we need to scroll up
   if (screenLocation.Row > NumberOfRows)
   {
       for (i = 0; i < NumberOfColumns * 2 * (NumberOfRows - 1); i++)
       {
           video_memory[i] = video_memory[i + (NumberOfColumns * 2)];
       }

       // Blank the last line
       for (i = (NumberOfRows - 1) * NumberOfColumns * 2; i < NumberOfRows * NumberOfColumns * 2; i += 2)
       {
           video_memory[i] = BLANK;
           video_memory[i + 1] = attributeByte;
       }

       screenLocation.Row = NumberOfRows;
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

// Prints out the status line string
void PrintStatusLine(char *string)
{
    unsigned char color = (COLOR_GREEN << 4) | (COLOR_BLACK & 0x0F);
    char *video_memory = (char *)VIDEO_MEMORY;
    int colStatusLine = 1;

    while (*string != '\0')
    {
        int offset = (25 - 1) * NumberOfColumns * 2 + (colStatusLine - 1) * 2;
        video_memory[offset] = *string;
        video_memory[offset + 1] = color;
        colStatusLine++;

        string++;
    }
}

// Prints a single character on the screen
void print_char(char character)
{
    char *video_memory = (char *)VIDEO_MEMORY;
    
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
            int offset = (screenLocation.Row - 1) * NumberOfColumns * 2 + (screenLocation.Col - 1) * 2;
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
void printf_long(unsigned long i, int base)
{
    char str[32] = "";
    ltoa(i, base, str);
    printf(str);
}