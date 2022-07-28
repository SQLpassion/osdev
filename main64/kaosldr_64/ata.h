#ifndef ATA_H
#define ATA_H

void ReadSectors(unsigned char *target_address, unsigned int LBA, unsigned char sector_count);
void WriteSectors(unsigned int LBA, unsigned char sector_count, unsigned int *source_address);
static void ATA_Wait_BSY();
static void ATA_Wait_DRQ();

#endif