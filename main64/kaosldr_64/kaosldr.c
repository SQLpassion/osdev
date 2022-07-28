#include "misc.h"

// Entry point of KLDR64.BIN
// The only task of the KLDR64.BIN file is to load the KERNEL.BIN file to the physical 
// memory address 0x100000 and execute it from there.
//
// These tasks must be done in KLDR64.BIN, because the CPU is now already in x64 Long Mode,
// and therefore we can access higher memory addresses like 0x100000.
// This would be not possible in KLDR16.BIN, because the CPU is here still in x16 Real Mode.
void kaosldr_main()
{
    // Initializes and clears the screen
    InitializeScreen();

    // Print a welcome message
    printf("Hello World from x64 Long Mode!\n");

    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock*)BIB_OFFSET;
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