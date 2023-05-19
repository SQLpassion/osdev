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

#endif