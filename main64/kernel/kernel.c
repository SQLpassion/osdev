#include "drivers/screen.h"
#include "drivers/keyboard.h"
#include "drivers/timer.h"
#include "isr/pic.h"
#include "isr/idt.h"
#include "kernel.h"
#include "common.h"
#include "date.h"
#include "memory.h"

// The main entry of our Kernel
void KernelMain()
{
    // Initialize the Kernel
    InitKernel();

    // Print out a welcome message
    SetColor(COLOR_LIGHT_BLUE);
    printf("Executing the x64 KAOS Kernel at the virtual address 0x");
    printf_long((unsigned long)&KernelMain, 16);
    printf("...\n");
    printf("===============================================================================\n\n");
    SetColor(COLOR_WHITE);

    // Print out the memory map that we have obtained from the BIOS
    // PrintMemoryMap();

    // Halt the system
    while (1 == 1) {}
}

// Initializes the whole Kernel
void InitKernel()
{
    // Initialize and clear the screen
    InitializeScreen(80, 24);

    // Initialize the physical Memory Manager
    InitMemoryManager();

    // Disable the hardware interrupts
    DisableInterrupts();

    // Initializes the PIC, and remap the IRQ handlers.
    // The 1st PIC handles the hardware interrupts 32 - 39 (input value 0x20).
    // The 2nd PIC handles the hardware interrupts 40 - 47 (input value 0x28).
    InitPic(0x20, 0x28);

    // Initialize the ISR & IRQ routines
    InitIdt();

    // Initialize the keyboard
    InitKeyboard();

    // Initialize the timer to fire every 1ms
    InitTimer(1000);
    
    // Enable the hardware interrupts again
    EnableInterrupts();
}

// Causes a Divide by Zero Exception
void DivideByZeroException()
{
    // This causes a Divide by Zero Exception - which calls the ISR0 routine
    int a = 5;
    int b = 0;
    int c = a / b;
}

// Tests the functionality of the keyboard
void KeyboardTest()
{
    char input[100] = "";

    printf("Please enter your name: ");
    scanf(input, 98);

    printf("Your name is ");
    printf(input);
    printf("\n");
}