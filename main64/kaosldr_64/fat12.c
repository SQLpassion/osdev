#include "misc.h"
#include "fat12.h"
#include "ata.h"

unsigned char *rootDirectoryBuffer = (unsigned char *)0x30000;

void LoadKernelIntoMemory()
{
    // Load the whole Root Directory (14 sectors) into memory
    ReadSectors(rootDirectoryBuffer, 19, 14);

    // Find the Root Directory Entry for KERNEL.BIN
    RootDirectoryEntry *entry = FindRootDirectoryEntry(KERNEL_IMAGE);

    if (entry != 0)
    {
        printf("\n");
        printf("Kernel found: ");
        printf(entry->Filename);
        printf("\n");
    }
}

// Finds a given Root Directory Entry by its Filename
RootDirectoryEntry* FindRootDirectoryEntry(char *Filename)
{
    RootDirectoryEntry *entry = (RootDirectoryEntry *)rootDirectoryBuffer;
    int i;

    for (i = 0; i < 16; i++)
    {
        printf(entry->Filename);
        printf("\n");

        if (entry->Filename[0] != 0x00)
        {
            // Check if we got the Root Directory Entry in which we are interested in
            if (strcmp(entry->Filename, Filename, 11) == 0)
                return entry;
        }

        // Move to the next Root Directory Entry
        entry = entry + 1;
    }

    // The requested Root Directory Entry was not found
    return 0;
}