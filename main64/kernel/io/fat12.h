#ifndef FAT12_H
#define FAT12_H

#define FAT_COUNT 2
#define EOF 0x0FF0
#define SECTORS_PER_FAT 9
#define RESERVED_SECTORS 1
#define ROOT_DIRECTORY_ENTRIES 224
#define BYTES_PER_SECTOR 512

// Represents a Root Directory Entry - 32 bytes long
struct RootDirectoryEntry
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
typedef struct RootDirectoryEntry RootDirectoryEntry;

// Load the given program into memory
int LoadProgram(unsigned char *Filename);

// Prints the Root Directory
void PrintRootDirectory();

// Finds a given Root Directory Entry by its Filename
RootDirectoryEntry* FindRootDirectoryEntry(unsigned char *Filename);

// Adds a new file to the FAT12 partition
void AddFile();

// Finds the next free Root Directory Entry
static RootDirectoryEntry *FindNextFreeRootDirectoryEntry();

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry);

// Loads the Root Directory into memory
static void LoadRootDirectory();

// Writes the Root Directory from the memory back to the disk
static void WriteRootDirectory();

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster);

#endif