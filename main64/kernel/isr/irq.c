#include "irq.h"
#include "idt.h"
#include "pic.h"
#include "../common.h"

// Defines an array for our various IRQ handlers
IRQ_HANDLER InterruptHandlers[IRQ_ENTRIES];

// Registers a IRQ callback function
void RegisterIrqHandler(int n, IRQ_HANDLER Handler)
{
    InterruptHandlers[n] = Handler;
}

// Common IRQ handler that is called as soon as an IRQ is raised
void IrqHandler(int InterruptNumber)
{
    // Signal that we have handled the received interrupt
    if (InterruptNumber >= 40)
    {
        // Send reset signal to slave
        outb(I86_PIC2_REG_COMMAND, I86_PIC_OCW2_MASK_EOI);
    }
    
    // Send reset signal to master
    outb(I86_PIC1_REG_COMMAND, I86_PIC_OCW2_MASK_EOI);

    // Call the IRQ callback function, if one is registered
    if (InterruptHandlers[InterruptNumber] != 0)
    {
        IRQ_HANDLER handler = InterruptHandlers[InterruptNumber];
        handler(InterruptNumber);
    }
}