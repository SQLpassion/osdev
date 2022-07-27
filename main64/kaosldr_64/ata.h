#ifndef ATA_H
#define ATA_H

void read_sectors_ATA_PIO(unsigned int *target_address, unsigned int LBA, unsigned char sector_count);
void write_sectors_ATA_PIO(unsigned int LBA, unsigned char sector_count, unsigned int *source_address);
static void ATA_wait_BSY();
static void ATA_wait_DRQ();

#endif