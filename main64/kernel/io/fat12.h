#ifndef FAT12_H
#define FAT12_H

// Represents a Root Directory Entry - 32 bytes long
struct RootDirectoryEntry
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
typedef struct RootDirectoryEntry RootDirectoryEntry;

// Prints the Root Directory
void PrintRootDirectory();

// Load the given program into memory
int LoadProgram(unsigned char *Filename);

// Reads the given file, and returns a pointer to the data in memory
unsigned char *ReadFile(unsigned char *FileName);

// Load all Clusters for the given Root Directory Entry into memory
static unsigned char *LoadFileIntoMemory(RootDirectoryEntry *Entry);

// Loads the Root Directory into memory
static void LoadRootDirectory();

// Finds a given Root Directory Entry by its Filename
static RootDirectoryEntry* FindRootDirectoryEntry(unsigned char *Filename);

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry);

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster);

#endif