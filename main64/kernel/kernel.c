#include "kernel.h"
#include "common.h"
#include "drivers/screen.h"
#include "drivers/keyboard.h"
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

    DisplayStatusLine();

    int i = 0;
    for (i = 0; i < 30; i++)
    {
        KeyboardTest();
    }

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

    // Initialize the keyboard
    InitKeyboard();
    
    // Enable the hardware interrupts again
    EnableInterrupts();
}

// Displays the BIOS Information Block
void DisplayStatusLine()
{
    char str[32] = "";

    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock*)BIB_OFFSET;

    // Set a green background color
    unsigned int color = (COLOR_GREEN << 4) | (COLOR_BLACK & 0x0F);
    int oldColor = SetColor(color);
    int oldRow = SetScreenRow(25);

    printf_noscrolling("Date: ");
    itoa(bib->Year, 10, str);
    printf_noscrolling(str);
    printf_noscrolling("-");

    itoa(bib->Month, 10, str);

    if (bib->Month < 10)
        printf_noscrolling("0");

    printf_noscrolling(str);
    printf_noscrolling("-");

    itoa(bib->Day, 10, str);
    printf_noscrolling(str);
    printf_noscrolling(", ");

    itoa(bib->Hour, 10, str);
    printf_noscrolling(str);
    printf_noscrolling(":");

    itoa(bib->Minute, 10, str);
    printf_noscrolling(str);
    printf_noscrolling(":");

    itoa(bib->Second, 10, str);
    printf_noscrolling(str);

    SetScreenRow(oldRow);
    SetColor(oldColor);
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
}