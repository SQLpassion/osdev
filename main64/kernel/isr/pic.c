#include "pic.h"
#include "../common.h"

// This code is based on http://www.brokenthorn.com/Resources/OSDevPic.html

// Initializes the PIC, and remaps the IRQs
void InitPic(unsigned char base0, unsigned char base1)
{
    unsigned char icw = 0;
    
    // Begin initialization of PIC
    icw = (icw & ~I86_PIC_ICW1_MASK_INIT) | I86_PIC_ICW1_INIT_YES;
    icw = (icw & ~I86_PIC_ICW1_MASK_IC4) | I86_PIC_ICW1_IC4_EXPECT;
    
    PicSendCommand(icw, 0);
    PicSendCommand(icw, 1);
    
    // Send initialization control word 2. This is the base addresses of the irq's
    PicSendData(base0, 0);
    PicSendData(base1, 1);
    
    // Send initialization control word 3. This is the connection between master and slave.
    // ICW3 for master PIC is the IR that connects to secondary pic in binary format
    // ICW3 for secondary PIC is the IR that connects to master pic in decimal format
    PicSendData(0x04, 0);
    PicSendData(0x02, 1);
    
    // Send Initialization control word 4.
    // Enables i86 mode.
    icw = (icw & ~I86_PIC_ICW4_MASK_UPM) | I86_PIC_ICW4_UPM_86MODE;
    PicSendData(icw, 0);
    PicSendData(icw, 1);
}

// Sends a command to the PICs
static void PicSendCommand(unsigned char cmd, unsigned char picNum)
{
    if (picNum > 1)
        return;
    
    unsigned char reg = (picNum == 1) ? I86_PIC2_REG_COMMAND : I86_PIC1_REG_COMMAND;
    outb(reg, cmd);
}

// Sends data to PICs
static void PicSendData(unsigned char data, unsigned char picNum)
{
    if (picNum > 1)
        return;
    
    unsigned char reg = (picNum == 1) ? I86_PIC2_REG_DATA : I86_PIC1_REG_DATA;
    outb(reg, data);
}

// Reads data from PICs
static unsigned char PicReadData(unsigned char picNum)
{
    if (picNum > 1)
        return 0;
    
    unsigned char reg = (picNum == 1) ? I86_PIC2_REG_DATA : I86_PIC1_REG_DATA;
    return inb(reg);
}