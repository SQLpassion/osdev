#include "common.h"
#include "drivers/screen.h"

void kernel_main()
{
    // Initializes and clears the screen
    InitializeScreen();

    // Print a welcome message
    printf("Executing the x64 OS Kernel at virtual address 0x");
    printf_long((unsigned long)&kernel_main, 16);
    printf("...\n");
    printf("\n");

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

    // Halt the system
    while (1 == 1) {}
}