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

// Represents a File Descriptor
struct FileDescriptor
{
    unsigned char FileName[11];
    unsigned char Extension[3];
    unsigned long FileSize;
    unsigned long CurrentFileOffset;
    char FileMode[2];
};
typedef struct FileDescriptor FileDescriptor;

// Initializes the FAT12 system
void InitFAT12();

// Opens an existing file in the FAT12 file system
unsigned long OpenFile(unsigned char *FileName, unsigned char *Extension, char *FileMode);

// Reads the requested data from a file into the provided buffer
unsigned long ReadFile(unsigned long FileHandle, unsigned char *Buffer, unsigned long Length);

// Writes the requested data from the provided buffer into a file
unsigned long WriteFile(unsigned long FileHandle, unsigned char *Buffer, unsigned long Length);

// Seeks to the specific position in the file
int SeekFile(unsigned long FileHandle, unsigned long NewFileOffset);

// Returns a flag if the file offset within the FileDescriptor has reached the end of file
int EndOfFile(unsigned long FileHandle);

// Closes a file in the FAT12 file system
int CloseFile(unsigned long FileHandle);

// Deletes an existing file in the FAT12 file system
int DeleteFile(unsigned char *FileName, unsigned char *Extension);

// Load the given program into memory
int LoadProgram(unsigned char *Filename);

// Prints the Root Directory
void PrintRootDirectory();

// Finds a given Root Directory Entry by its Filename
RootDirectoryEntry* FindRootDirectoryEntry(unsigned char *Filename);

// Prints out the FileDescriptorList entries
void PrintFileDescriptorList();

// Prints out the FAT12 chain
void PrintFATChain();

// Tests some functionality of the FAT12 file system
void FAT12Test();

// Creates a new file in the FAT12 file system
static void CreateFile(unsigned char *FileName, unsigned char *Extension);

// Allocates a new cluster to the given FAT sector
static unsigned short AllocateNewClusterToFile(unsigned short CurrentFATSect);

// Deallocates the FAT clusters for a file - beginning with the given first cluster
static void DeallocateFATClusters(unsigned short FirstCluster);

// Loads the Root Directory into memory
static void LoadRootDirectory();

// Finds the next free Root Directory Entry
static RootDirectoryEntry *FindNextFreeRootDirectoryEntry();

// Writes the Root Directory and the FAT12 tables from the memory back to the disk
static void WriteRootDirectoryAndFAT();

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster);

// Writes the provided value to the specific FAT12 cluster
static void FATWrite(unsigned short Cluster, unsigned short Value);

// Finds the next free FAT12 cluster entry
static unsigned short FindNextFreeFATEntry();

// Sets the last Access Date and the last Write Date for the RootDirectoryEntry
static void SetLastAccessDate(RootDirectoryEntry *Entry);

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry);

// Calculates a Hash Value for the given file name
static unsigned long HashFileName(unsigned char *FileName);

#endif