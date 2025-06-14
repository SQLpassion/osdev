#ifndef COMMON_H
#define COMMON_H

#include "memory/physical-memory.h"

// Defines the NULL pointer
#define NULL ((void *) 0)

// The physical memory offset where the BIOS Information Block is stored
#define BIB_OFFSET 0x1000

// The physical memory offset where the KERNEL.BIN file was loaded
#define KERNEL_OFFSET 0x100000

#define SERIAL_PORT_COM1 0x3F8

// This structure stores all the information that we retrieve from the BIOS while we are in x16 Real Mode
typedef struct BiosInformationBlock
{
    int Year;
    short Month;
    short Day;
    short Hour;
    short Minute;
    short Second;

    // The number of Memory Map Entries that the BIOS reported
    short MemoryMapEntries;

    // The maximum physical RAM reported by the BIOS
    long MaxMemory;

    // The current available physical Page Frames (managed by the Physical Memory Manager)
    long AvailablePageFrames;

    PhysicalMemoryLayout *PhysicalMemoryLayout;
} BiosInformationBlock;

// Reads a single char (8 bytes) from the specified port
unsigned char inb(unsigned short Port);

// Reads a single short (16 bytes) from the specific port
unsigned short inw(unsigned short Port);

// Reads a single int (32 bytes) from the specific port
unsigned int inl(unsigned short Port);

// Writes a single char (8 bytes) to the specified port
void outb(unsigned short Port, unsigned char Value);

// Writes a single short (16 bytes) to the specified port
void outw(unsigned short Port, unsigned short Value);

// Writes a single int (32 bytes) to the specified port
void outl(unsigned short Port, unsigned int Value);

// A simple memset implementation
void *memset(void *s, int c, long n);

// A simple memcpy implementation
void memcpy(void *dest, void *src, int len);

// Returns the length of the given string
int strlen(char *string);

// A simple strcpy implementation
char *strcpy(char *destination, const char *source);

// A simple strcmp implementation
int strcmp(char *s1, char *s2);

// A simple strcat implementation
char *strcat(char *destination, char *source);

// Returns a substring from a given string
int substring(char *source, int from, int n, char *target);

// Returns the position of the specific character in the given string
int find(char *string, char junk);

// Checks if a string starts with a given prefix
int startswith(char *string, char *prefix);

// Converts a string to upper case
void toupper(char *s);

// Converts a string to lower case
void tolower(char *s);

// Converts an integer value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void itoa(unsigned int i, unsigned base, char *buf);

// Helper function for the itoa function.
static void itoa_helper(unsigned int i, unsigned base, char *buf);

// Converts a long value to a string value for a specific base (base 10 => decimal, base 16 => hex)
void ltoa(unsigned long i, unsigned base, char *buf);

// Helper function for the ltoa function.
static void ltoa_helper(unsigned long i, unsigned base, char *buf);

// Converts an ASCII string to its integer value
int atoi(char *str);

int isalpha(char c);

int isdigit(char c);

int isalnum(char c);

int is_identifier(char c);

int is_number(char c);

int strncmp(const char *s1, const char *s2, int n);

char *strtok(char *str, const char *delim);

char *strchr(const char *s, int c);

char *strdup(const char *s);

// Formats an Integer value with a leading zero.
void FormatInteger(int Value, char *Buffer);

// Formats a Hex string with the given number of leading zeros.
void FormatHexString(char *string, int length);

// Aligns the Number to the given Alignment.
int AlignNumber(int Number, int Alignment);

// Sets the given Bit in the provided Bitmap mask.
void SetBit(unsigned long Bit, unsigned long *BitmapMask);

// Clears the given Bit in the provided Bitmap mask.
void ClearBit(unsigned long Bit, unsigned long *BitmapMask);

// Tests if a given Bit is set in the provided Bitmap mask.
int TestBit(unsigned long Bit, unsigned long *BitmapMask);

// Initializes the Serial Port COM1
void InitSerialPort();

// Checks if the Transmission Buffer is empty
int IsTansmissionBufferEmpty();

// Writes a single character to the Serial Port
void WriteCharToSerialPort(char a);

// Writes a null-terminated string to the Serial Port
void WriteStringToSerialPort(char *string);

#endif