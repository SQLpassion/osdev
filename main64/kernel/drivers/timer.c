#include "../common.h"
#include "../isr/irq.h"
#include "../date.h"
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

    // Refresh the status line
    RefreshStatusLine();
}

// IRQ callback function
static void TimerCallback(int Number)
{
    // Increment the clock counter
    counter++;

    if (counter % 1000 == 0)
    {
        // Increment the system date by 1 second
        IncrementSystemDate();

        // Refresh the status line
        RefreshStatusLine();
    }
}

// Refreshs the status line
void RefreshStatusLine()
{
    char buffer[80] = "";
    char str[32] = "";
    char tmp[2] = "";

    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

    // Print out the year
    itoa(bib->Year, 10, str);
    strcat(buffer, str);
    strcat(buffer, "-");

    // Print out the month
    FormatInteger(bib->Month, tmp);
    strcat(buffer, tmp);
    strcat(buffer, "-");

    // Print out the day
    FormatInteger(bib->Day, tmp);
    strcat(buffer, tmp);
    strcat(buffer, ", ");

    // Print out the hour
    FormatInteger(bib->Hour, tmp);
    strcat(buffer, tmp);
    strcat(buffer, ":");

    // Print out the minute
    FormatInteger(bib->Minute, tmp);
    strcat(buffer, tmp);
    strcat(buffer, ":");

    // Print out the second
    FormatInteger(bib->Second, tmp);
    strcat(buffer, tmp);

    // Pad the remaining columns with a blank, so that the status line goes
    // over the whole row
    int len = 80 - strlen(buffer);

    while (len > 0)
    {
        strcat(buffer, " ");
        len--;
    }

    // Print out the status line
    PrintStatusLine(buffer);
}