#include "misc.h"
#include "ata.h"

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

    unsigned int *buffer = (unsigned int *)0x1000;
    read_sectors_ATA_PIO(buffer, 2, 1);

    int i = 0;
    while (i < 256)
    {
        print_char(buffer[i] & 0xFF);
        print_char((buffer[i] >> 8) & 0xFF);
        i++;
    }

    write_sectors_ATA_PIO(2, 1, (unsigned int *)0x7C00);

    // Halt the system
    while (1 == 1) {}
}