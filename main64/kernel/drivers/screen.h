#ifndef SCREEN_H
#define SCREEN_H

// Video output memory address
#define VIDEO_MEMORY 0xFFFF8000000B8000

// The number of rows of the video memory
#define ROWS 25

// The number of columns of the video memory
#define COLS 80

#define CRLF '\n'
#define TAB '\t'

// Text mode color constants
enum VGA_Color
{
    COLOR_BLACK = 0,
    COLOR_BLUE = 1,
    COLOR_GREEN = 2,
    COLOR_CYAN = 3,
    COLOR_RED = 4,
    COLOR_MAGENTA = 5,
    COLOR_BROWN = 6,
    COLOR_LIGHT_GREY = 7,
    COLOR_DARK_GREY = 8,
    COLOR_LIGHT_BLUE = 9,
    COLOR_LIGHT_GREEN = 10,
    COLOR_LIGHT_CYAN = 11,
    COLOR_LIGHT_RED = 12,
    COLOR_LIGHT_MAGENTA = 13,
    COLOR_LIGHT_BROWN = 14,
    COLOR_WHITE = 15
};

// This struct contains information about the screen
typedef struct ScreenLocation
{
    // The current row on the screen
    int Row;

    // The current column on the screen
    int Col;

    // The used attributes
    int Attributes;
} ScreenLocation;

// Initializes the screen
void InitializeScreen();

// Sets the specific color
int SetColor(int Color);

// Returns the current cursor position
void GetCursorPosition(int *Row, int *Col);

// Sets the current cursor position
void SetCursorPosition(int Row, int Col);

// Moves the screen cursor to the current location on the screen
void MoveCursor();

// Clears the screen
void ClearScreen();

// Scrolls the screen, when we have used more than 25 rows
void Scroll();

// Prints a null-terminated string
void printf(char *string);

// Prints a single character
void print_char(char character);

// Prints out an integer value
void printf_int(int i, int base);

// Prints out a long value
void printf_long(long i, int base);

#endif