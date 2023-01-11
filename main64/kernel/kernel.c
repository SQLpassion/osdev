#include "kernel.h"
#include "common.h"
#include "drivers/screen.h"
#include "isr/pic.h"
#include "isr/idt.h"

// The main entry of our Kernel
void KernelMain()
{
    // Initialize the Kernel
    InitKernel();

    // Print out a welcome message
    printf("Executing the x64 KAOS Kernel at virtual address 0x");
    printf_long((unsigned long)&KernelMain, 16);
    printf("...\n");
    printf("\n");

    // Display the BIOS Information Block
    DisplayBiosInformationBlock();

    // Causes a Divide by Zero Exception
    DivideByZeroException();

    // Halt the system
    while (1 == 1) {}
}

// Initializes the whole Kernel
void InitKernel()
{
    // Initialize and clear the screen
    InitializeScreen();

    // Disable the hardware interrupts
    DisableInterrupts();

    // Initializes the PIC, and remap the IRQ handlers.
    // The 1st PIC handles the hardware interrupts 32 - 39 (input value 0x20).
    // The 2nd PIC handles the hardware interrupts 40 - 47 (input value 0x28).
    InitPic(0x20, 0x28);

    // Initialize the ISR & IRQ routines
    InitIdt();
    
    // Enable the hardware interrupts again
    EnableInterrupts();
}

// Displays the BIOS Information Block
void DisplayBiosInformationBlock()
{
    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock*)BIB_OFFSET;
    printf("Getting the BIOS Information Block:\n");
    printf("Year: ");
    printf_int(bib->Year, 10);
    printf("\n");
    printf("Month: ");
    printf_int(bib->Month, 10);
    printf("\n");
    printf("Day: ");
    printf_int(bib->Day, 10);
    printf("\n");
    printf("Hour: ");
    printf_int(bib->Hour, 10);
    printf("\n");
    printf("Minute: ");
    printf_int(bib->Minute, 10);
    printf("\n");
    printf("Second: ");
    printf_int(bib->Second, 10);
    printf("\n");
}

// Causes a Divide by Zero Exception
void DivideByZeroException()
{
    // This causes a Divide by Zero Exception - which calls the ISR0 routine
    int a = 5;
    int b = 0;
    int c = a / b;
}