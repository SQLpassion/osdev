#ifndef FAT12_H
#define FAT12_H

// Represents a Root Directory Entry
struct _RootDirectoryEntry
{
    unsigned char FileName[8];
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

// Loads the given Kernel file into memory
int LoadKernelIntoMemory(char *FileName);

// Finds a given Root Directory Entry by its Filename
static RootDirectoryEntry* FindRootDirectoryEntry(char *FileName);

// Load all Clusters for the given Root Directory Entry into memory
static int LoadFileIntoMemory(RootDirectoryEntry *Entry);

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster);

#endif