#include "common.h"
#include "drivers/screen.h"

char tbuf[64];
char tbuf_long[64];
char bchars[] = {'0','1','2','3','4','5','6','7','8','9','A','B','C','D','E','F'};

// Reads a single char (8 bytes) from the specified port
unsigned char inb(unsigned short Port)
{
   unsigned char ret;
   asm volatile("inb %1, %0" : "=a" (ret) : "dN" (Port));
   
   return ret;
}

// Reads a single short (16 bytes) from the specific port
unsigned short inw(unsigned short Port)
{
   unsigned short ret;
   asm volatile ("inw %1, %0" : "=a" (ret) : "dN" (Port));
   
   return ret;
}

// Reads a single int (32 bytes) from the specific port
unsigned int inl(unsigned short Port)
{
   unsigned int ret;
   asm volatile ("inl %1, %0" : "=a" (ret) : "dN" (Port));
   
   return ret;
}

// Writes a single char (8 bytes) to the specified port
void outb(unsigned short Port, unsigned char Value)
{
    asm volatile ("outb %1, %0" : : "dN" (Port), "a" (Value));
}

// Writes a single short (16 bytes) to the specified port
void outw(unsigned short Port, unsigned short Value)
{
    asm volatile ("outw %1, %0" : : "dN" (Port), "a" (Value));
}

// Writes a single int (32 bytes) to the specified port
void outl(unsigned short Port, unsigned int Value)
{
    asm volatile ("outl %1, %0" : : "dN" (Port), "a" (Value));
}
// A simple memset implementation
void *memset(void *s, int c, long n)
{
    unsigned char *p = s;
    
    while (n--)
        *p++ = (unsigned char)c;
    
    return s;
}

// A simple memcpy implementation
void memcpy(void *dest, void *src, int len)
{
    int i;
    char *csrc = (char *)src;
    char *cdest = (char *)dest;

    for (i = 0; i < len; i++)
    {
        cdest[i] = csrc[i];
    }
}

// Returns the length of the given string
int strlen(char *string)
{
    int len = 0;

    while (*string != '\0')
    {
        len++;
        string++;
    }

    return len;
}

// A simple strcpy implementation
char *strcpy(char *destination, const char *source)
{
    // return if no memory is allocated to the destination
    if (destination == 0x0)
        return 0x0;

    // take a pointer pointing to the beginning of destination string
    char *ptr = destination;
    
    // copy the C-string pointed by source into the array pointed by destination
    while (*source != '\0')
    {
        *destination = *source;
        destination++;
        source++;
    }

    // include the terminating null character
    *destination = '\0';

    // destination is returned by standard strcpy()
    return ptr;
}

// A simple strcmp implementation
int strcmp(char *s1, char *s2)
{
    int i = 0;
    int len = strlen(s2);

    while (*s1 && (*s1 == *s2) && i < len)
    {
        s1++;
        s2++;
        i++;
    }

    return *(unsigned char *)s1 - *(unsigned char *)s2;
}

// A simple strcat implementation
char *strcat(char *destination, char *source)
{
    // "ptr" points to the end of the destination string
    char *ptr = destination + strlen(destination);
 
    // Add the source characters to the destination string
    while (*source != '\0')
    {
        *ptr++ = *source++;
    }
 
    // Null terminate the destination string
    *ptr = '\0';
 
    // Return the final string
    return destination;
}

// Returns a substring from a given string
int substring(char *source, int from, int n, char *target)
{
    int length,i;
    //get string length 
    for(length=0;source[length]!='\0';length++);
     
    if(from>length){
        printf("Starting index is invalid.\n");
        return 1;
    }
     
    if((from+n)>length){
        //get substring till end
        n=(length-from);
    }
     
    //get substring in target
    for(i=0;i<n;i++){
        target[i]=source[from+i];
    }
    target[i]='\0'; //assign null at last
     
    return 0;    
}

// Returns the position of the specific character in the given string
int find(char *string, char junk)
{
    int pos = 0;

    while (*string != junk)
    {
        pos++;
        string++;
    }

    return pos;
}

// Checks if a string starts with a given prefix
int startswith(char *string, char *prefix)
{
    while (*prefix)
    {
        if (*prefix++ != *string++)
            return 0;
    }

    return 1;
}

// Converts a string to upper case
void toupper(char *s)
{
    for (; *s; s++)
        if (('a' <= *s) && (*s <= 'z'))
            *s = 'A' + (*s - 'a');
}

// Converts a string to lower case
void tolower(char *s)
{
    for(; *s; s++)
        if(('A' <= *s) && (*s <= 'Z'))
            *s = 'a' + (*s - 'A');
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

// Helper function for the itoa function.
static void itoa_helper(unsigned int i, unsigned base, char *buf)
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

// Converts an ASCII string to its integer value
int atoi(char *str)
{
    int res = 0;
    int i;

    for (i = 0; str[i] != '\0'; ++i)
    {
        res = res * 10 + str[i] - '0';
    }

    return res;
}

// Formats an Integer value with a leading zero.
void FormatInteger(int Value, char *Buffer)
{
    char str[32] = "";

    // Empty the buffer
    strcpy(Buffer, "");

    // Add a leading zero - if necessary
    if (Value < 10)
    {
        strcat(Buffer, "0");
    }

    // Add the integer value
    itoa(Value, 10, str);
    strcat(Buffer, str);
}

// Formats a Hex string with the given number of leading zeros.
void FormatHexString(char *string, int length)
{
    int oldLength = strlen(string);
    int diff = length - oldLength;
    char newString[32] = "";

    if (diff > 0)
    {
        while (diff > 0)
        {
            strcat(newString, "0");
            diff--;
        }
    }

    strcat(newString, string);
    strcpy(string, newString);
}

// Aligns the Number to the given Alignment.
int AlignNumber(int Number, int Alignment)
{
    int alignedNumber = Number / Alignment;

    if (alignedNumber * Alignment < Number)
        alignedNumber++;

    return alignedNumber * Alignment;
}

// Sets the given Bit in the provided Bitmap mask
void SetBit(unsigned long Bit, unsigned long *BitmapMask)
{
    BitmapMask[Bit / 64] |= ((unsigned long)1 << (Bit % 64));
}

// Clears the given Bit in the provided Bitmap mask
void ClearBit(unsigned long Bit, unsigned long *BitmapMask)
{
    BitmapMask[Bit / 64] &= ~((unsigned long)1 << (Bit % 64));
}

// Tests if a given Bit is set in the provided Bitmap mask
int TestBit(unsigned long Bit, unsigned long *BitmapMask)
{
    unsigned long result = BitmapMask[Bit / 64] & ((unsigned long)1 << (Bit % 64));

    if (result > 0)
        return 1;
    else
        return 0;
}

// Initializes the Serial Port COM1
void InitSerialPort()
{
    outb(SERIAL_PORT_COM1 + 1, 0x00);    // Disable all interrupts
    outb(SERIAL_PORT_COM1 + 3, 0x80);    // Enable DLAB (set baud rate divisor)
    outb(SERIAL_PORT_COM1 + 0, 0x03);    // Set divisor to 3 (lo byte) 38400 baud
    outb(SERIAL_PORT_COM1 + 1, 0x00);    //                  (hi byte)
    outb(SERIAL_PORT_COM1 + 3, 0x03);    // 8 bits, no parity, one stop bit
    outb(SERIAL_PORT_COM1 + 2, 0xC7);    // Enable FIFO, clear them, with 14-byte threshold
    outb(SERIAL_PORT_COM1 + 4, 0x0B);    // IRQs enabled, RTS/DSR set
    outb(SERIAL_PORT_COM1 + 4, 0x1E);    // Set in loopback mode, test the serial chip
    outb(SERIAL_PORT_COM1 + 0, 0xAE);    // Test serial chip (send byte 0xAE and check if serial returns same byte)
 
    // Check if serial is faulty (i.e: not same byte as sent)
    if(inb(SERIAL_PORT_COM1 + 0) != 0xAE) {
      return;
    }
 
    // If serial is not faulty set it in normal operation mode
    // (not-loopback with IRQs enabled and OUT#1 and OUT#2 bits enabled)
    outb(SERIAL_PORT_COM1 + 4, 0x0F);
}

// Checks if the Transmission Buffer is empty
int IsTansmissionBufferEmpty()
{
   return inb(SERIAL_PORT_COM1 + 5) & 0x20;
}

// Writes a single character to the Serial Port
void WriteCharToSerialPort(char a)
{
    while (IsTansmissionBufferEmpty() == 0);
 
    outb(SERIAL_PORT_COM1, a);
}

// Writes a null-terminated string to the Serial Port
void WriteStringToSerialPort(char *string)
{
    while (*string != '\0')
    {
        WriteCharToSerialPort(*string);
        string++;
    }
}