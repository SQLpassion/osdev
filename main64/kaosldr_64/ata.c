#include "misc.h"
#include "ata.h"

#define STATUS_BSY 0x80
#define STATUS_RDY 0x40
#define STATUS_DRQ 0x08
#define STATUS_DF  0x20
#define STATUS_ERR 0x01

void ReadSectors(unsigned char *target_address, unsigned int LBA, unsigned char sector_count)
{
    ATA_Wait_BSY();
    outb(0x1F2, sector_count);
    outb(0x1F3, (unsigned char) LBA);
    outb(0x1F4, (unsigned char)(LBA >> 8));
    outb(0x1F5, (unsigned char)(LBA >> 16)); 
    outb(0x1F6, 0xE0 | ((LBA >> 24) & 0xF));
    outb(0x1F7, 0x20);

    for (int j =0; j < sector_count; j++)
    {
        ATA_Wait_BSY();
        ATA_Wait_DRQ();

        for (int i = 0; i < 256; i++)
        {
            // Retrieve a single 16-byte value from the input port
            unsigned short read_buffer = inw(0x1F0);

            // Write the 2 retrieved 8-byte values to their target address
            target_address[i] = read_buffer & 0xFF;
            target_address++;
            target_address[i] = (read_buffer >> 8) & 0xFF;
        }
        
        target_address += 512;
    }
}

void WriteSectors(unsigned int LBA, unsigned char sector_count, unsigned int *source_address)
{
    ATA_Wait_BSY();
    outb(0x1F6, 0xE0 | ((LBA >>24) & 0xF));
    outb(0x1F2, sector_count);
    outb(0x1F3, (unsigned char) LBA);
    outb(0x1F4, (unsigned char)(LBA >> 8));
    outb(0x1F5, (unsigned char)(LBA >> 16)); 
    outb(0x1F7, 0x30);

    for (int j = 0; j < sector_count; j++)
    {
        ATA_Wait_BSY();
        ATA_Wait_DRQ();

        for (int i = 0; i < 256; i++)
        {
            outl(0x1F0, source_address[i]);
        }
    }
}

static void ATA_Wait_BSY()   //Wait for bsy to be 0
{
    while (inb(0x1F7) & STATUS_BSY);
}


static void ATA_Wait_DRQ()  //Wait fot drq to be 1
{
    while (!(inb(0x1F7) & STATUS_RDY));
}