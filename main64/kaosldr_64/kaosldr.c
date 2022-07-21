#include "misc.h"

void kaosldr_main()
{
    // Initializes and clears the screen
    InitializeScreen();

    // Print a welcome message
    printf("Hello World from x64 Long Mode!\n");

    // Halt the system
    while (1 == 1) {}   
}