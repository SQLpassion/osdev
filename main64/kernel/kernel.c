#include "drivers/screen.h"
#include "drivers/keyboard.h"
#include "drivers/timer.h"
#include "memory/physical-memory.h"
#include "memory/virtual-memory.h"
#include "memory/heap.h"
#include "multitasking/multitasking.h"
#include "multitasking/gdt.h"
#include "isr/pic.h"
#include "isr/idt.h"
#include "io/fat12.h"
#include "kernel.h"
#include "common.h"
#include "date.h"

// The main entry of our Kernel
void KernelMain(int KernelSize)
{
    // Initialize the Kernel
    InitKernel(KernelSize);

    /* // Print out a welcome message
    SetColor(COLOR_LIGHT_BLUE);
    printf("Executing the x64 KAOS Kernel at the virtual address 0x");
    printf_long((unsigned long)&KernelMain, 16);
    printf("...\n");
    printf("===============================================================================\n\n");
    SetColor(COLOR_WHITE); */

    // Halt the system
    while (1 == 1) {}
}

// Initializes the whole Kernel
void InitKernel(int KernelSize)
{
    // Initialize and clear the screen
    InitializeScreen(80, 24);

    // Disable the hardware interrupts
    DisableInterrupts();

    // Initialize the physical Memory Manager
    InitPhysicalMemoryManager(KernelSize);

    // Initialize the virtual Memory Manager
    InitVirtualMemoryManager(0);

    // Initializes the PIC, and remap the IRQ handlers.
    // The 1st PIC handles the hardware interrupts 32 - 39 (input value 0x20).
    // The 2nd PIC handles the hardware interrupts 40 - 47 (input value 0x28).
    InitPic(0x20, 0x28);

    // Initialize the ISR & IRQ routines
    InitIdt();

    // Initialize the keyboard
    InitKeyboard();

    // Initialize the timer to fire every 4ms
    InitTimer(250);
    
    // Enable the hardware interrupts again
    EnableInterrupts();

    // Initialize the Heap.
    // It generates Page Faults, therefore the interrupts must be already re-enabled.
    InitHeap();

    // Initializes the GDT and TSS structures
    InitGdt();
    
    // Create the initial OS tasks
    CreateInitialTasks();

    // Refresh the status line
    RefreshStatusLine();

    // Initializes the FAT12 system
    InitFAT12();

    // Register the Context Switching IRQ Handler when the Timer fires
    InitTimerForContextSwitching();

    /* char *buffer = (char *)malloc(510);
    unsigned long fileHandle = OpenFile("BIGFILE ", "TXT");

    while (!EndOfFile(fileHandle))
    {
        ReadFile(fileHandle, buffer, 500);
        printf(buffer);
    }

    CloseFile(fileHandle); */
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