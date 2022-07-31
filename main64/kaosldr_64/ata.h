#ifndef ATA_H
#define ATA_H

#define STATUS_BSY 0x80
#define STATUS_RDY 0x40
#define STATUS_DRQ 0x08
#define STATUS_DF  0x20
#define STATUS_ERR 0x01

// Reads a given number of disk sectors (512 bytes) from the starting LBA address into the target memory address.
void ReadSectors(unsigned char *TargetAddress, unsigned int LBA, unsigned char SectorCount);

// Writes a given number of disk sectors (512 bytes) to the starting LBA address of the disk from the source memory address.
void WriteSectors(unsigned int *SourceAddress, unsigned int LBA, unsigned char SectorCount);

// Waits until the BSY flag is cleared.
static void WaitForBSYFlag();

// Waits until the DRQ flag is set.
static void WaitForDRQFlag();

#endif