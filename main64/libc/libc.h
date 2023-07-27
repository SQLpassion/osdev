#ifndef LIBC_H
#define LIBC_H

#define KEY_RETURN      '\r'
#define KEY_BACKSPACE   '\b'

// Prints out a null-terminated string
void printf(unsigned char *string);

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

// Deletes the file in the FAT12 file system
int DeleteFile(unsigned char *FileName, unsigned char *Extension);

// Opens the requested file in the FAT12 file system
unsigned long OpenFile(unsigned char *FileName, unsigned char *Extension, const char *FileMode);

// Closes thef ile in the FAT12 file system
int CloseFile(unsigned long FileHandle);

// Reads the requested data from a file into the provided buffer
unsigned long ReadFile(unsigned long FileHandle, unsigned char *Buffer, unsigned long Length);

// Writes the requested data from the provided buffer into a file
unsigned long WriteFile(unsigned long FileHandle, unsigned char *Buffer, unsigned long Length);

// Seeks to the specific position in the file
int SeekFile(unsigned long FileHandle, unsigned long NewFileOffset);

// Returns a flag if the file offset within the FileDescriptor has reached the end of file
int EndOfFile(unsigned long FileHandle);

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