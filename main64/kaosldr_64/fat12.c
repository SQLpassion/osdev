#include "misc.h"
#include "fat12.h"
#include "ata.h"

const int EOF = 0x0FF0;

unsigned char *rootDirectoryBuffer = (unsigned char *)0x30000;
unsigned char *fatBuffer = (unsigned char *)0x31C00;
unsigned char *kernelBuffer = (unsigned char *)0xFFFF800000100000;

// Loads the given Kernel file into memory
int LoadKernelIntoMemory(char *FileName)
{
    // Load the whole Root Directory (14 sectors) into memory
    ReadSectors(rootDirectoryBuffer, 19, 14);

    // Find the Root Directory Entry for KERNEL.BIN
    RootDirectoryEntry *entry = FindRootDirectoryEntry(FileName);

    if (entry != NULL)
    {
        // Load the whole FAT (18 sectors) into memory
        ReadSectors(fatBuffer, 1, 18);

        // Load the Kernel into memory
        return LoadFileIntoMemory(entry);
    }
    else
    {
        // The Kernel was not found on the disk
        printf("The requested Kernel file ");
        printf(FileName);
        printf(" was not found.");
        printf("\n");

        // Halt the system
        while (1 == 1) {}
    }
}

// Finds a given Root Directory Entry by its Filename
static RootDirectoryEntry* FindRootDirectoryEntry(char *FileName)
{
    RootDirectoryEntry *entry = (RootDirectoryEntry *)rootDirectoryBuffer;
    int i;

    for (i = 0; i < 16; i++)
    {
        if (entry->FileName[0] != 0x00)
        {
            // Check if we got the Root Directory Entry in which we are interested in
            if (strcmp(entry->FileName, FileName, 11) == 0)
                return entry;
        }

        // Move to the next Root Directory Entry
        entry = entry + 1;
    }

    // The requested Root Directory Entry was not found
    return NULL;
}

// Load all Clusters for the given Root Directory Entry into memory
static int LoadFileIntoMemory(RootDirectoryEntry *Entry)
{
    int sectorCount = 0;

    // Read the first cluster of the file into memory
    ReadSectors(kernelBuffer, Entry->FirstCluster + 33 - 2, 1);
    sectorCount++;
    unsigned short nextCluster = FATRead(Entry->FirstCluster);

    // Read the whole file into memory until we reach the EOF mark
    while (nextCluster < EOF)
    {
        kernelBuffer += 512;
        ReadSectors(kernelBuffer, nextCluster + 33 - 2, 1);
        sectorCount++;
        
        // Read the next Cluster from the FAT table
        nextCluster = FATRead(nextCluster);
    }

    // Return the number of read sectors
    return sectorCount;
}

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster)
{
    // Calculate the offset into the FAT table
    unsigned int fatOffset = (Cluster / 2) + Cluster;
    unsigned long *offset = fatBuffer + fatOffset;

    // Read the entry from the FAT
    unsigned short val = *offset;
   
    if (Cluster & 0x0001)
    {
        // Odd Cluster
        return val >> 4; // Highest 12 Bits
    }
    else
    {
        // Even Cluster
        return val & 0x0FFF; // Lowest 12 Bits
    }
}