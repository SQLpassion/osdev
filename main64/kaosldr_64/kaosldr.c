#include "misc.h"

// Entry point of the x64 based KAOSLDR
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