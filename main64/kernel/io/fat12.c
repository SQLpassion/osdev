#include "fat12.h"
#include "ata.h"
#include "../common.h"
#include "../memory/heap.h"
#include "../drivers/screen.h"

const int NumberOfFATs = 2;
const int SectorsPerFAT = 9;
const int SectorsPerCluster = 1;
const int ReservedSectors = 1;
const int RootDirectoryEntries = 224;
const int BytesPerSector = 512;
unsigned char *PROGRAM_BUFFER = (char *)0xFFFF8000FFFF0000;
unsigned char *ROOT_DIRECTORY_BUFFER;
unsigned char *FAT_BUFFER;
const int EOF = 0x0FF0;
int RootDirectoryLoaded = 0;

// Prints the Root Directory
void PrintRootDirectory()
{
    char str[32] = "";
    int fileCount = 0;
    int fileSize = 0;
    int i;

    // Check, if the Root Directory is already loaded into memory
    if (RootDirectoryLoaded == 0)
    {
        LoadRootDirectory();
        RootDirectoryLoaded = 1;
    }

    RootDirectoryEntry *entry = (RootDirectoryEntry *)ROOT_DIRECTORY_BUFFER;

    for (i = 0; i < 16; i++)
    {
        if (entry->Filename[0] != 0x00)
        {
            // Print out the file size
            itoa(entry->FileSize, 10, str);
            printf(str);
            printf(" bytes");
            printf("\t");

            // Extract the name and the extension
            char name[9] = "";
            char extension[4] = "";
            substring(entry->Filename, 0, 8, name);
            substring(entry->Filename, 8, 3, extension);

            // Convert everything to lower case
            tolower(name);
            tolower(extension);

            // Print out the file name
            int pos = find(name, ' ');
            char trimmedName[9] = "";
            substring(name, 0, pos, trimmedName);
            printf(trimmedName);
            printf(".");
            printf(extension);
            printf("\n");

            // Calculate the file count and the file size
            fileCount++;
            fileSize += entry->FileSize;
        }

        // Move to the next Root Directory Entry
        entry = entry + 1;
    }

    // Print out the file count and the file size
    printf("\t\t");
    itoa(fileCount, 10, str);
    printf(str);
    printf(" File(s)");
    printf("\t");
    itoa(fileSize, 10, str);
    printf(str);
    printf(" bytes");
    printf("\n");
}

// Reads the given file, and returns a pointer to the data in memory
unsigned char *ReadFile(unsigned char *FileName)
{
    // Check, if the Root Directory is already loaded into memory
    if (RootDirectoryLoaded == 0)
    {
        LoadRootDirectory();
        RootDirectoryLoaded = 1;
    }
    
    // Find the Root Directory Entry for the given program name
    RootDirectoryEntry *entry = FindRootDirectoryEntry(FileName);

    if (entry != 0)
    {
        return LoadFileIntoMemory(entry);
    }
    else
    {
        return 0x0;
    }
}

// Load all Clusters for the given Root Directory Entry into memory
static unsigned char *LoadFileIntoMemory(RootDirectoryEntry *Entry)
{
    // We add a whole sector size (512 bytes) to the allocated memory, because we only read whole sectors
    unsigned char *ptr = malloc(Entry->FileSize + BytesPerSector);
    unsigned char *beginning = ptr;
    
    // Read the first cluster of the file into memory
    ReadSectors((unsigned char *)ptr, Entry->FirstCluster + 33 - 2, 1);
    unsigned short nextCluster = FATRead(Entry->FirstCluster);

    // Read the whole file into memory until we reach the EOF mark
    while (nextCluster < EOF)
    {
        ptr = ptr + BytesPerSector;
        ReadSectors((unsigned char *)ptr, nextCluster + 33 - 2, 1);
        
        // Read the next Cluster from the FAT table
        nextCluster = FATRead(nextCluster);
    }

    return beginning;
}

// Load the given program into memory
int LoadProgram(unsigned char *Filename)
{
    // Check, if the Root Directory is already loaded into memory
    if (RootDirectoryLoaded == 0)
    {
        LoadRootDirectory();
        RootDirectoryLoaded = 1;
    }
    
    // Find the Root Directory Entry for the given program name
    RootDirectoryEntry *entry = FindRootDirectoryEntry(Filename);

    if (entry != 0)
    {
        LoadProgramIntoMemory(entry);
        return 1;
    }
    else
    {
        return 0;
    }
}

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry)
{
    // Read the first cluster of the Kernel into memory
    unsigned char *program_buffer = (unsigned char *)PROGRAM_BUFFER;
    ReadSectors((unsigned char *)program_buffer, Entry->FirstCluster + 33 - 2, 1);
    unsigned short nextCluster = FATRead(Entry->FirstCluster);

    // Read the whole file into memory until we reach the EOF mark
    while (nextCluster < EOF)
    {
        program_buffer = program_buffer + BytesPerSector;
        ReadSectors((unsigned char *)program_buffer, nextCluster + 33 - 2, 1);

        // Read the next Cluster from the FAT table
        nextCluster = FATRead(nextCluster);
    }
}

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster)
{
    // Calculate the offset into the FAT table
    unsigned int fatOffset = (Cluster / 2) + Cluster;
    unsigned long *offset = FAT_BUFFER + fatOffset;
    
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

// Loads the Root Directory and the FAT into memory
static void LoadRootDirectory()
{
    // Calculate the Root Directory Size: 14 sectors: => 32 * 224 / 512
    short rootDirectorySectors = 32 * RootDirectoryEntries / BytesPerSector;

    // Calculate the LBA address of the Root Directory: 19: => 2 * 9 + 1
    short lbaAddressRootDirectory = NumberOfFATs * SectorsPerFAT + ReservedSectors;

    // Load the whole Root Directory (14 sectors) into memory
    ROOT_DIRECTORY_BUFFER = malloc(rootDirectorySectors * BytesPerSector);

    ReadSectors((unsigned char *)ROOT_DIRECTORY_BUFFER, lbaAddressRootDirectory, rootDirectorySectors);

    // Load the whole FAT (18 sectors) into memory
    FAT_BUFFER = malloc(NumberOfFATs * SectorsPerFAT * BytesPerSector);
    ReadSectors((unsigned char *)FAT_BUFFER, 1, NumberOfFATs * SectorsPerFAT);
}

// Finds a given Root Directory Entry by its Filename
static RootDirectoryEntry* FindRootDirectoryEntry(unsigned char *Filename)
{
    // Check, if the Root Directory is already loaded into memory
    if (RootDirectoryLoaded == 0)
    {
        LoadRootDirectory();
        RootDirectoryLoaded = 1;
    }

    RootDirectoryEntry *entry = (RootDirectoryEntry *)ROOT_DIRECTORY_BUFFER;
    int i;

    for (i = 0; i < 16; i++)
    {
        if (entry->Filename[0] != 0x00)
        {
            // Extract the first 8 bytes, which include the file name
            char name[8] = "";
            substring(entry->Filename, 0, 8, name);

            // Convert the provided file name to upper case, because the
            // Root Directory Entries are also stored in upper case
            toupper(Filename);

            printf(name);

            // Check if we got the Root Directory Entry in which we are interested in
            int pos = find(name, ' ');
            char trimmedName[9] = "";
            substring(name, 0, pos, trimmedName);

            if (strcmp(trimmedName, Filename) == 0)
                return entry;
        }

        // Move to the next Root Directory Entry
        entry = entry + 1;
    }

    // while (1 == 1);

    // The requested Root Directory Entry was not found
    return 0;
}