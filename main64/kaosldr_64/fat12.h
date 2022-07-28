#ifndef FAT12_H
#define FAT12_H

// The Kernel image name to be loaded into memory
#define KERNEL_IMAGE "KLDR64  BIN"

// Represents a Root Directory Entry
struct _RootDirectoryEntry
{
    unsigned char Filename[8];
    unsigned char Extension[3];
    unsigned char Attributes[1];
    unsigned char Reserved[2];
    unsigned char CreationTime[2];
    unsigned char CreationDate[2];
    unsigned char LastAccessDate[2];
    unsigned char Ignore[2];
    unsigned char LastWriteTime[2];
    unsigned char LastWriteDate[2];
    unsigned short FirstCluster;
    unsigned int FileSize;
} __attribute__ ((packed));
typedef struct _RootDirectoryEntry RootDirectoryEntry;

void LoadKernelIntoMemory();

// Finds a given Root Directory Entry by its Filename
RootDirectoryEntry* FindRootDirectoryEntry(char *Filename);

#endif