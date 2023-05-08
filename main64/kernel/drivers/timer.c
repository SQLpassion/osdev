#include "../common.h"
#include "../isr/irq.h"
#include "../date.h"
#include "../kernel.h"
#include "timer.h"
#include "screen.h"

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
}