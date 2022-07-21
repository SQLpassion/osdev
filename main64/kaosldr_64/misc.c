#include "misc.h"

// Define a variable for the screen location information
ScreenLocation screenLocation;

// Writes a single char (8 bytes) to the specified port.
void outb(unsigned short Port, unsigned char Value)
{
	asm volatile ("outb %1, %0" : : "dN" (Port), "a" (Value));
}

// Initializes the screen.
void InitializeScreen()
{
	screenLocation.Row = 1;
	screenLocation.Col = 1;
	screenLocation.Attributes = COLOR_LIGHT_MAGENTA;
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