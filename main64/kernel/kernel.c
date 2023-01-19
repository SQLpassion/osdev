#include "kernel.h"
#include "common.h"
#include "date.h"
#include "drivers/screen.h"
#include "drivers/keyboard.h"
#include "drivers/timer.h"
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

    DumpMemoryMap();

    // Set a custom system date
    // SetDate(2023, 2, 28);
    // SetTime(22, 40, 3);
    
    /* int i = 0;
    for (i = 0; i < 30; i++)
    {
        KeyboardTest();
    } */

    // Halt the system
    while (1 == 1) {}
}

// Initializes the whole Kernel
void InitKernel()
{
    // Initialize and clear the screen
    InitializeScreen(80, 24);

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

// 3218997248

// Dumps out the Memory Map
void DumpMemoryMap()
{
	MemoryRegion *region = (MemoryRegion *)0x8004;
	char str[32] = "";
	int i;
    int *length = (int *)0x8000;

	printf("Detected Memory Map:");
	printf("\n");
	printf("=============================================================================");
	printf("\n");

	// for (i = 0; i < *length; i++)
    for (i = 0; i < 12; i++)
	{
        printf("Type: ");
		ltoa(region[i].Type, 16, str);
		printf(str);
        printf("  ");

		ltoa(region[i].Start, 16, str);
		printf("Start: 0x");
		printf(str);
        printf("\t");

		if (strlen(str) < 6)
		{
			printf("\t");
		}

        printf("End: 0x");
		ltoa(region[i].Start + region[i].Size - 1, 16, str);
		printf(str);
        printf("\t");

        if (strlen(str) < 9)
		{
			printf("\t");
		}

        printf("Size: 0x");
        ltoa(region[i].Size, 16, str);
        printf(str);
        printf("\t");

        if (strlen(str) < 9)
		{
			// printf("\t");
		}

        /* printf("Type: ");
		ltoa(region[i].Type, 16, str);
		printf(str); */

		// printf(" (");
		// printf(MemoryRegionType[region[i].Type - 1]);
		// printf(")");

		printf("\n");
	}
}