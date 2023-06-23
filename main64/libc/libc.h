#ifndef LIBC_H
#define LIBC_H

#define KEY_RETURN      '\r'
#define KEY_BACKSPACE   '\b'

// Prints out a null-terminated string
void printf(char *string);

// Returns the PID of the current executing process
long GetPID();

// Terminates the current executing process
void TerminateProcess();

// Returns the entered character
char getchar();

// Reads a string with the given size from the keyboard, and returns it
void scanf(char *buffer, int buffer_size);

// Returns the current cursor position
void GetCursorPosition(int *Row, int *Col);

// Sets the current cursor position
void SetCursorPosition(int *Row, int *Col);

// Executes the given User Mode program
int ExecuteUserModeProgram(unsigned char *FileName);

// Prints out the root directory of the FAT12 partition
int PrintRootDirectory();

// Clears the screen
int ClearScreen();

// Creates a new file in the FAT12 file system
int CreateFile(unsigned char *FileName, unsigned char *Extension, unsigned char *InitialContent);

// Prints out the given file name
int PrintFile(unsigned char *FileName, unsigned char *Extension);

// Prints out an integer value
void printf_int(int i, int base);

// Prints out a long value
void printf_long(unsigned long i, int base);

// Converts an integer value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void itoa(unsigned int i, unsigned base, char *buf);

// Converts a long value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void ltoa(unsigned long i, unsigned base, char *buf);

// Helper function for the itoa function.
static void itoa_helper(unsigned int i, unsigned base, char *buf);

// Helper function for the ltoa function.
static void ltoa_helper(unsigned long i, unsigned base, char *buf);

// Checks if a string starts with a given prefix
int StartsWith(char *string, char *prefix);

#endif