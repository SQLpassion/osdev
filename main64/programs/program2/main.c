#include "syscalls/syscall.h"

void outb(unsigned short Port, unsigned char Value);

void main()
{
    // This function call will trigger a GP fault, because the
    // code runs in Ring 3 (User Mode)
    // outb(0x3D4, 14);

    while (1 == 1)
    {
        SYSCALL1(SYSCALL_PRINTF, "Hello World from User Mode Program #2...\n");
    }
}

// The x64 out assembly instructions are only allowed in Ring 0 code.
// Therefore, this instruction will cause a GP fault, because the code
// runs in Ring 3 (User Mode)
void outb(unsigned short Port, unsigned char Value)
{
    asm volatile ("outb %1, %0" : : "dN" (Port), "a" (Value));
}