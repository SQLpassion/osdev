#include "../common.h"
#include "../isr/irq.h"
#include "timer.h"
#include "screen.h"

int counter = 0;

// Initializes the hardware timer
void InitTimer(int Hertz)
{
    int divisor = 1193180 / Hertz;
    
    // Send the command byte
    outb(0x43, 0x36);
    
    // Divisor has to be sent byte-wise, so split here into upper/lower bytes
    unsigned char l = (unsigned char)(divisor & 0xFF);
    unsigned char h = (unsigned char)((divisor >> 8));
    
    // Send the frequency divisor
    outb(0x40, l);
    outb(0x40, h);

    // Registers the IRQ callback function for the hardware timer
    RegisterIrqHandler(32, &TimerCallback);
}

// IRQ callback function
static void TimerCallback(int Number)
{
    char str[32] = "";
    counter++;

    if (counter % 1000 == 0)
    {
        // Getting a reference to the BIOS Information Block
        BiosInformationBlock *bib = (BiosInformationBlock*)BIB_OFFSET;

        // Set a green background color
        unsigned int color = (COLOR_GREEN << 4) | (COLOR_BLACK & 0x0F);
        int oldColor = SetColor(color);
        int oldRow = SetScreenRow(25);

        itoa(bib->Year, 10, str);
        printf_noscrolling(str);
        printf_noscrolling("-");

        itoa(bib->Month, 10, str);

        if (bib->Month < 10)
            printf_noscrolling("0");

        printf_noscrolling(str);
        printf_noscrolling("-");

        itoa(bib->Day, 10, str);

        if (bib->Day < 10)
            printf_noscrolling("0");

        printf_noscrolling(str);
        printf_noscrolling(", ");

        itoa(bib->Hour, 10, str);

        if (bib->Hour < 10)
            printf_noscrolling("0");

        printf_noscrolling(str);
        printf_noscrolling(":");

        itoa(bib->Minute, 10, str);

        if (bib->Minute < 10)
            printf_noscrolling("0");

        printf_noscrolling(str);
        printf_noscrolling(":");

        itoa(bib->Second, 10, str);

        if (bib->Second < 10)
            printf_noscrolling("0");

        printf_noscrolling(str);

        itoa(counter / 1000, 10, str);
        printf_noscrolling("    Seconds since startup: ");
        printf_noscrolling(str);

        SetScreenRow(oldRow);
        SetColor(oldColor);
    }
}