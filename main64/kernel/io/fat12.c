#include "fat12.h"
#include "ata.h"
#include "../common.h"
#include "../memory/heap.h"
#include "../drivers/screen.h"

// The addresses where the Root Directory and the FAT tables are stored.
// The memory regions will be allocated on the Heap.
unsigned char *ROOT_DIRECTORY_BUFFER;
unsigned char *FAT_BUFFER;

// The virtual memory address where the user program will be loaded.
unsigned char *EXECUTABLE_BASE_ADDRESS = (unsigned char *)0x0000700000000000;

// This flag stores if the Root Directory was already loaded into memory.
int RootDirectoryLoaded = 0;

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

    for (i = 0; i < ROOT_DIRECTORY_ENTRIES; i++)
    {
        if (entry->FileName[0] != 0x00)
        {
            // Print out the file size
            itoa(entry->FileSize, 10, str);
            printf(str);
            printf(" bytes");
            printf("\t");

            // Start Cluster
            itoa(entry->FirstCluster, 10, str);
            printf("Start Cluster: ");
            printf(str);
            printf("\t");

            // Extract the name and the extension
            char name[9] = "";
            char extension[4] = "";
            substring(entry->FileName, 0, 8, name);
            substring(entry->FileName, 8, 3, extension);

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

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry)
{
    // Read the first cluster of the Kernel into memory
    unsigned char *program_buffer = (unsigned char *)EXECUTABLE_BASE_ADDRESS;
    ReadSectors((unsigned char *)program_buffer, Entry->FirstCluster + 33 - 2, 1);
    unsigned short nextCluster = FATRead(Entry->FirstCluster);

    // Read the whole file into memory until we reach the EOF mark
    while (nextCluster < EOF)
    {
        program_buffer = program_buffer + BYTES_PER_SECTOR;
        ReadSectors(program_buffer, nextCluster + 33 - 2, 1);

        // Read the next Cluster from the FAT table
        nextCluster = FATRead(nextCluster);
    }
}

// Loads the Root Directory and the FAT into memory
static void LoadRootDirectory()
{
    // Calculate the Root Directory Size: 14 sectors: => 32 * 224 / 512
    short rootDirectorySectors = 32 * ROOT_DIRECTORY_ENTRIES / BYTES_PER_SECTOR;

    // Calculate the LBA address of the Root Directory: 19: => 2 * 9 + 1
    short lbaAddressRootDirectory = FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS;

    // Load the whole Root Directory (14 sectors) into memory
    ROOT_DIRECTORY_BUFFER = malloc(rootDirectorySectors * BYTES_PER_SECTOR);

    ReadSectors((unsigned char *)ROOT_DIRECTORY_BUFFER, lbaAddressRootDirectory, rootDirectorySectors);

    // Load the whole FAT (18 sectors) into memory
    FAT_BUFFER = malloc(FAT_COUNT * SECTORS_PER_FAT * BYTES_PER_SECTOR);
    ReadSectors((unsigned char *)FAT_BUFFER, 1, FAT_COUNT * SECTORS_PER_FAT);
}

// Writes the Root Directory from the memory back to the disk
static void WriteRootDirectory()
{
    // Calculate the Root Directory Size: 14 sectors: => 32 * 224 / 512
    short rootDirectorySectors = 32 * ROOT_DIRECTORY_ENTRIES / BYTES_PER_SECTOR;

    // Calculate the LBA address of the Root Directory: 19: => 2 * 9 + 1
    short lbaAddressRootDirectory = FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS;

    // Write the Root Directory
    WriteSectors((unsigned int *)ROOT_DIRECTORY_BUFFER, lbaAddressRootDirectory, rootDirectorySectors);
    
    // Write the FAT table
    WriteSectors((unsigned int *)FAT_BUFFER, 1, FAT_COUNT * SECTORS_PER_FAT);
}

// Finds a given Root Directory Entry by its Filename
RootDirectoryEntry* FindRootDirectoryEntry(unsigned char *FileName)
{
    // Check, if the Root Directory is already loaded into memory
    if (RootDirectoryLoaded == 0)
    {
        LoadRootDirectory();
        RootDirectoryLoaded = 1;
    }

    RootDirectoryEntry *entry = (RootDirectoryEntry *)ROOT_DIRECTORY_BUFFER;
    int i;

    for (i = 0; i < ROOT_DIRECTORY_ENTRIES; i++)
    {
        if (entry->FileName[0] != 0x00)
        {
            if (strcmp(FileName, entry->FileName) == 0)
                return entry;
        }

        // Move to the next Root Directory Entry
        entry = entry + 1;
    }

    // The requested Root Directory Entry was not found
    return 0;
}

// Adds a new file to the FAT12 partition
void AddFile()
{
    char input[10] = "";

    PrintRootDirectory();
    printf("\n");

    /* unsigned short nextFreeCluster = FindNextFreeFATEntry();
    printf("\n");
    printf("Next useable Cluster: ");
    printf_int(nextFreeCluster, 10);
    printf("\n");

    FATWrite(nextFreeCluster, 0xABC);

    nextFreeCluster = FindNextFreeFATEntry();
    printf("\n");
    printf("Next useable Cluster: ");
    printf_int(nextFreeCluster, 10);
    printf("\n");

    FATWrite(nextFreeCluster, 0xDEF); */

    printf("Please enter a new file name: ");
    scanf(input, 8);

    RootDirectoryEntry *freeEntry = FindNextFreeRootDirectoryEntry();

    if (freeEntry != 0x0)
    {
        unsigned short nextFreeCluster = FindNextFreeFATEntry();
        FATWrite(nextFreeCluster, 0xFF0);

        strcpy(freeEntry->FileName, input);
        strcpy(freeEntry->Extension, "TXT");
        freeEntry->FileSize = 124;
        freeEntry->FirstCluster = nextFreeCluster;
    }

    PrintRootDirectory();
    printf("\n");

    printf("Before WriteRootDirectory()...\n");
    WriteRootDirectory();

    printf("After WriteRootDirectory()...\n");
}

// Finds the next free Root Directory Entry
static RootDirectoryEntry *FindNextFreeRootDirectoryEntry()
{
    // Check, if the Root Directory is already loaded into memory
    if (RootDirectoryLoaded == 0)
    {
        LoadRootDirectory();
        RootDirectoryLoaded = 1;
    }

    RootDirectoryEntry *entry = (RootDirectoryEntry *)ROOT_DIRECTORY_BUFFER;

    for (int i = 0; i < ROOT_DIRECTORY_ENTRIES; i++)
    {
        if (entry->FileName[0] == 0x00)
            return entry;

        // Move to the next Root Directory Entry
        entry = entry + 1;
    }

    // A free Root Directory Entry was not found
    return 0x0;
}

// Reads the next FAT Entry from the FAT Tables
static unsigned short FATRead(unsigned short Cluster)
{
    // Calculate the offset into the FAT table
    unsigned int fatOffset = (Cluster / 2) + Cluster;

    // CAUTION!
    // The following line generates a warning during the compilation ("incompatible-pointer-types").
    // But we can't cast the right side to "(unsigned long *)", because then the loader component
    // will not work anymore!
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

unsigned short FindNextFreeFATEntry()
{
    // unsigned short Cluster = 2;
    unsigned short Cluster = 1;
    unsigned short result = 1;

    while (result > 0)
    {
        Cluster++;

        // Calculate the offset into the FAT table
        unsigned int fatOffset = (Cluster / 2) + Cluster;
        unsigned long *offset = FAT_BUFFER + fatOffset;

        // Read the entry from the FAT
        unsigned short val = *offset;

        if (Cluster & 0x0001)
        {
            // Odd Cluster
            result = val >> 4; // Highest 12 Bits

            /* printf_int(result, 10);
            printf("\n");

            if (result == 4095)
                printf("\n"); */
        }
        else
        {
            // Even Cluster
            result = val & 0x0FFF; // Lowest 12 Bits

            /* printf_int(result, 10);
            printf("\n");

            if (result == 4095)
                printf("\n"); */
        }

        // Cluster++;
    }

    return Cluster;
}

/*
03 F0 FF
|| || ||
11 21 22

003 FFF

Pos: 
Even: 4, 1, 2
Odd:  5, 6, 3

ABC DEF:
BC FA DE
*/
void FATWrite(unsigned short Cluster, unsigned short Value)
{
    // Calculate the offset into the FAT table
    unsigned int fatOffset = (Cluster / 2) + Cluster;
   
    if (Cluster % 2 == 0)
    {
        printf("Even\n");

        printf_long(FAT_BUFFER[fatOffset + 0], 16);
        printf("\n");
        printf_long(FAT_BUFFER[fatOffset + 1], 16);
        printf("\n");
        printf("\n");


        FAT_BUFFER[fatOffset + 0] = (0xff & Value);
        FAT_BUFFER[fatOffset + 1] = ((0xf0 & (FAT_BUFFER[fatOffset + 1])) |  (0x0f & (Value >> 8)));

        printf_long(FAT_BUFFER[fatOffset + 0], 16);
        printf("\n");
        printf_long(FAT_BUFFER[fatOffset + 1], 16);
        printf("\n");
        printf("\n");
    }
    else
    {
        printf("Odd\n");

        printf_long(FAT_BUFFER[fatOffset - 1], 16);
        printf("\n");
        printf_long(FAT_BUFFER[fatOffset + 0], 16);
        printf("\n");
        printf_long(FAT_BUFFER[fatOffset + 1], 16);
        printf("\n");
        printf("\n");

        FAT_BUFFER[fatOffset + 0] = ((0x0f & (FAT_BUFFER[fatOffset + 0])) | ((0x0f & Value) << 4));
        FAT_BUFFER[fatOffset + 1] = ((0xff) & (Value >> 4));

        printf_long(FAT_BUFFER[fatOffset - 1], 16);
        printf("\n");
        printf_long(FAT_BUFFER[fatOffset + 0], 16);
        printf("\n");
        printf_long(FAT_BUFFER[fatOffset + 1], 16);
        printf("\n");
    }
}