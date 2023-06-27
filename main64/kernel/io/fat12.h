#ifndef FAT12_H
#define FAT12_H

#define EOF                     0x0FF0
#define BYTES_PER_SECTOR        512
#define FAT_COUNT               2
#define SECTORS_PER_FAT         9
#define RESERVED_SECTORS        1
#define ROOT_DIRECTORY_ENTRIES  224
#define DATA_AREA_BEGINNING     31
#define FAT1_CLUSTER            1
#define FAT2_CLUSTER            10

#define FAT12_YEAROFFSET        1980

// Represents a Root Directory Entry - 32 bytes long
struct RootDirectoryEntry
{
    unsigned char FileName[8];
    unsigned char Extension[3];
    unsigned char Attributes[1];
    unsigned char Reserved[2];
    unsigned CreationSecond: 5;
    unsigned CreationMinute: 6;
    unsigned CreationHour: 5;
    unsigned CreationDay: 5;
    unsigned CreationMonth: 4;
    unsigned CreationYear: 7;
    unsigned LastAccessDay: 5;
    unsigned LastAccessMonth: 4;
    unsigned LastAccessYear: 7;
    unsigned char Ignore[2];
    unsigned LastWriteSecond: 5;
    unsigned LastWriteMinute: 6;
    unsigned LastWriteHour: 5;
    unsigned LastWriteDay: 5;
    unsigned LastWriteMonth: 4;
    unsigned LastWriteYear: 7;
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

// Creates a new file in the FAT12 file system
void CreateFile(unsigned char *FileName, unsigned char *Extension, unsigned char *InitialContent);

// Deletes an existing file in the FAT12 file system
void DeleteFile(unsigned char *FileName, unsigned char *Extension);

// Prints out the given file
void PrintFile(unsigned char *FileName, unsigned char *Extension);


// Prints out the FAT12 chain
void PrintFATChain();

// Deallocates the FAT clusters for a file - beginning with the given first cluster
static void DeallocateFATClusters(unsigned short FirstCluster);

// Finds the next free Root Directory Entry
static RootDirectoryEntry *FindNextFreeRootDirectoryEntry();

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster);

// Writes the provided value to the specific FAT12 cluster
static void FATWrite(unsigned short Cluster, unsigned short Value);

// Finds the next free FAT12 cluster entry
static unsigned short FindNextFreeFATEntry();

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry);

// Loads the Root Directory into memory
static void LoadRootDirectory();

// Writes the Root Directory and the FAT12 tables from the memory back to the disk
static void WriteRootDirectoryAndFAT();

#endif