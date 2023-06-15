#include "ata.h"
#include "../common.h"

// Reads a given number of disk sectors (512 bytes) from the starting LBA address into the target memory address.
void ReadSectors(unsigned char *TargetAddress, unsigned int LBA, unsigned char SectorCount)
{
    WaitForBSYFlag();

    outb(0x1F2, SectorCount);
    outb(0x1F3, (unsigned char) LBA);
    outb(0x1F4, (unsigned char)(LBA >> 8));
    outb(0x1F5, (unsigned char)(LBA >> 16)); 
    outb(0x1F6, 0xE0 | ((LBA >> 24) & 0xF));
    outb(0x1F7, 0x20);

    for (int j =0; j < SectorCount; j++)
    {
        WaitForBSYFlag();
        WaitForDRQFlag();

        for (int i = 0; i < 256; i++)
        {
            // Retrieve a single 16-byte value from the input port
            unsigned short readBuffer = inw(0x1F0);

            // Write the 2 retrieved 8-byte values to their target address
            TargetAddress[i] = readBuffer & 0xFF;
            TargetAddress++;
            TargetAddress[i] = (readBuffer >> 8) & 0xFF;
        }
        
        TargetAddress += 512;
    }
}

// Writes a given number of disk sectors (512 bytes) to the starting LBA address of the disk from the source memory address.
void WriteSectors(unsigned int *SourceAddress, unsigned int LBA, unsigned char SectorCount)
{
    WaitForBSYFlag();

    outb(0x1F2, SectorCount);
    outb(0x1F3, (unsigned char) LBA);
    outb(0x1F4, (unsigned char)(LBA >> 8));
    outb(0x1F5, (unsigned char)(LBA >> 16)); 
    outb(0x1F6, 0xE0 | ((LBA >>24) & 0xF));
    outb(0x1F7, 0x30);

    for (int j = 0; j < SectorCount; j++)
    {
        WaitForBSYFlag();
        WaitForDRQFlag();

        for (int i = 0; i < 256; i++)
        {
            outl(0x1F0, SourceAddress[i]);
        }

        SourceAddress += 256;
    }
}

// Waits until the BSY flag is cleared.
static void WaitForBSYFlag()
{
    while (inb(0x1F7) & STATUS_BSY);
}

// Waits until the DRQ flag is set.
static void WaitForDRQFlag()
{
    while (!(inb(0x1F7) & STATUS_DRQ));
}