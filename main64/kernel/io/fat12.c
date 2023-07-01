#include "fat12.h"
#include "ata.h"
#include "../common.h"
#include "../list.h"
#include "../memory/heap.h"
#include "../drivers/screen.h"
#include "../multitasking/multitasking.h"

// The addresses where the Root Directory and the FAT tables are stored.
// The memory regions will be allocated on the Heap.
unsigned char *ROOT_DIRECTORY_BUFFER;
unsigned char *FAT_BUFFER;

// The virtual memory address where the user program will be loaded.
unsigned char *EXECUTABLE_BASE_ADDRESS_PTR = (unsigned char *)0x0000700000000000;

// Stores the File Descriptors for all opened files
List *FileDescriptorList = 0x0;

// Initializes the FAT12 system
void InitFAT12()
{
    FileDescriptorList = NewList();
    FileDescriptorList->PrintFunctionPtr = &PrintFileDescriptorList;

    // Load the RootDirectory and the FAT tables into memory
    LoadRootDirectory();
}

// Load the given program into memory
int LoadProgram(unsigned char *Filename)
{
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

// Finds a given Root Directory Entry by its Filename
RootDirectoryEntry* FindRootDirectoryEntry(unsigned char *FileName)
{
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

// Creates a new file in the FAT12 file system
void CreateFile(unsigned char *FileName, unsigned char *Extension, unsigned char *InitialContent)
{
    // Find the next free RootDirectoryEntry
    RootDirectoryEntry *freeEntry = FindNextFreeRootDirectoryEntry();

    if (freeEntry != 0x0)
    {
        // Getting a reference to the BIOS Information Block
        BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

        // Allocate the first cluster for the new file
        unsigned short startCluster = FindNextFreeFATEntry();
        FATWrite(startCluster, 0xFFF);

        strcpy(freeEntry->FileName, FileName);
        strcpy(freeEntry->Extension, Extension);
        freeEntry->FileSize = strlen(InitialContent);
        freeEntry->FirstCluster = startCluster;

        // Set the Date/Time information of the new file
        freeEntry->LastWriteYear = freeEntry->LastAccessYear = freeEntry->CreationYear = bib->Year - FAT12_YEAROFFSET;
        freeEntry->LastWriteMonth = freeEntry->LastAccessMonth = freeEntry->CreationMonth = bib->Month;
        freeEntry->LastWriteDay =  freeEntry->LastAccessDay = freeEntry->CreationDay = bib->Day;
        freeEntry->LastWriteHour = freeEntry->CreationHour = bib->Hour;
        freeEntry->LastWriteMinute = freeEntry->CreationMinute = bib->Minute;
        freeEntry->LastWriteSecond = freeEntry->CreationSecond = bib->Second / 2;

        // Write the changed Root Directory and the FAT tables back to disk
        WriteRootDirectoryAndFAT();

        // Allocate a new cluster of 512 bytes in memory, and copy the initial content into it.
        // Therefore, we can make sure that the remaining bytes are all zeroed out.
        unsigned char *content = (unsigned char *)malloc(BYTES_PER_SECTOR);
        memset(content, 0x0, BYTES_PER_SECTOR);
        strcpy(content, InitialContent);

        // Write the intial file content to disk
        WriteSectors((unsigned int *)content, startCluster + DATA_AREA_BEGINNING, 1);

        // Release the block of memory
        free(content);
    }
}

// Deletes an existing file in the FAT12 file system
void DeleteFile(unsigned char *FileName, unsigned char *Extension)
{
    // Construct the full file name
    char fullFileName[11];
    strcpy(fullFileName, FileName);
    strcat(fullFileName, Extension);
   
    // Find the Root Directory Entry for the given program name
    RootDirectoryEntry *entry = FindRootDirectoryEntry(fullFileName);

    if (entry != 0x0)
    {
        // Deallocate the FAT entries for the file
        DeallocateFATClusters(entry->FirstCluster);

        // Deallocate the RootDirectoryEntry of the file
        memset(entry, 0x0, sizeof(RootDirectoryEntry));

        // Write everything back to disk
        WriteRootDirectoryAndFAT();
    }
}

// Opens an existing file in the FAT12 file system
unsigned long OpenFile(unsigned char *FileName, unsigned char *Extension)
{
    char pid[10] = "";

    // Construct the full file name
    char fullFileName[15];
    strcpy(fullFileName, FileName);
    strcat(fullFileName, Extension);
   
    // Find the Root Directory Entry for the given program name
    RootDirectoryEntry *entry = FindRootDirectoryEntry(fullFileName);

    if (entry != 0x0)
    {
        // The PID of the current running task is concatenated to the file name
        // to make it unique across multiple running tasks.
        // Otherwise we would have a hash collision if the same file is opened across
        // multiple running tasks.
        tolower(fullFileName);
        ltoa(GetTaskState()->PID, 10, pid);
        strcat(fullFileName, pid);

        // Calculate a hash value for the given file name
        unsigned long hashValue = HashFileName(fullFileName);
        
        // Create a new FileDescriptor and store it in the system-wide Kernel list "FileDescriptorList"
        FileDescriptor *descriptor = (FileDescriptor *)malloc(sizeof(FileDescriptor));
        strcpy(descriptor->FileName, FileName);
        strcpy(descriptor->Extension, Extension);
        descriptor->FileSize = entry->FileSize;
        descriptor->CurrentFileOffset = 0;
        AddEntryToList(FileDescriptorList, descriptor, hashValue);
      
        // Return the key of the newly added FileDescriptor
        return hashValue;
    }

    return 0;
}

// Closes a file in the FAT12 file system
void CloseFile(unsigned long FileHandle)
{
    // Find the file which needs to be closed
    ListEntry *descriptor = GetEntryFromList(FileDescriptorList, FileHandle);

    if (descriptor != 0x0)
    {
        // Close the file by removing it from the list
        RemoveEntryFromList(FileDescriptorList, descriptor);
    }
}

// Reads the requested data from a file into the provided buffer
void ReadFile(unsigned long FileHandle, unsigned char *Buffer, unsigned long Length)
{
    // Find the file from which we want to read
    ListEntry *entry = GetEntryFromList(FileDescriptorList, FileHandle);
    FileDescriptor *descriptor = (FileDescriptor *)entry->Payload;

    // Zero-Initialize the target buffer
    memset(Buffer, 0x0, Length);

    // The requested data can't be longer than a physical disk sector
    if (Length > BYTES_PER_SECTOR)
        return;

    if (descriptor != 0x0)
    {
        // Construct the full file name
        char fullFileName[11];
        strcpy(fullFileName, descriptor->FileName);
        strcat(fullFileName, descriptor->Extension);

        // Find the Root Directory Entry for the given program name
        RootDirectoryEntry *entry = FindRootDirectoryEntry(fullFileName);

        if (entry != 0x0)
        {
            // Allocate a file buffer
            unsigned char *file_buffer = (unsigned char *)malloc(BYTES_PER_SECTOR);
            
            // Calculate from the current file position the cluster and the offset within that cluster
            unsigned long cluster = descriptor->CurrentFileOffset / BYTES_PER_SECTOR;
            unsigned long offsetWithinCluster = descriptor->CurrentFileOffset - (cluster * BYTES_PER_SECTOR);
            unsigned short fatSector = entry->FirstCluster;
    
            // Loop until we reach the cluster that we want to read
            for (int i = 0; i < cluster; i++)
            {
                // Read the next Cluster from the FAT table
                fatSector = FATRead(fatSector);
            }

            // Calculate the following disk sector
            unsigned short fatSectorFollowing = FATRead(fatSector);

            // Check for the EndOfFile condition
            if (descriptor->CurrentFileOffset + Length > descriptor->FileSize)
                Length = descriptor->FileSize - descriptor->CurrentFileOffset;

            // Read the specific sector from disk
            ReadSectors((unsigned char *)file_buffer, fatSector + DATA_AREA_BEGINNING, 1);

            // We also read the following sector, when the requested data is stored across 2 disk sectors
            if (offsetWithinCluster + Length > BYTES_PER_SECTOR)
                ReadSectors((unsigned char *)file_buffer + BYTES_PER_SECTOR, fatSectorFollowing + DATA_AREA_BEGINNING, 1);

            // Copy the requested data into the destination buffer
            memcpy(Buffer, file_buffer + offsetWithinCluster, Length);
           
            // Set the current file position within the FileDescriptor
            descriptor->CurrentFileOffset += Length;

            // Release the file buffer
            free(file_buffer);
        }
    }
}

// Writes the requested data from the provided buffer into a file
int WriteFile(unsigned long FileHandle, unsigned char *Buffer, unsigned long Length)
{
    // Find the file from which we want to read
    ListEntry *entry = GetEntryFromList(FileDescriptorList, FileHandle);
    FileDescriptor *descriptor = (FileDescriptor *)entry->Payload;

    // The data can't be longer than a physical disk sector
    if (Length > BYTES_PER_SECTOR)
        return 0;

    if (descriptor != 0x0)
    {
        // Construct the full file name
        char fullFileName[11];
        strcpy(fullFileName, descriptor->FileName);
        strcat(fullFileName, descriptor->Extension);

        // Find the Root Directory Entry for the given program name
        RootDirectoryEntry *entry = FindRootDirectoryEntry(fullFileName);

        if (entry != 0x0)
        {
            // Allocate a file buffer
            unsigned char *file_buffer = (unsigned char *)malloc(BYTES_PER_SECTOR);

            // Calculate from the current file position the cluster and the offset within that cluster
            unsigned long cluster = descriptor->CurrentFileOffset / BYTES_PER_SECTOR;
            unsigned long offsetWithinCluster = descriptor->CurrentFileOffset - (cluster * BYTES_PER_SECTOR);
            unsigned short currentFatSector = entry->FirstCluster;

            // Loop until we reach the cluster that we want to write to.
            // If necessary, new clusters will be created and added for the file.
            for (int i = 0; i < cluster; i++)
            {
                // Read the next Cluster from the FAT table
                unsigned short nextFatSector = FATRead(currentFatSector);

                // The next cluster is the last one in the chain
                if (nextFatSector >= EOF)
                {
                    // Allocate a new cluster for the file
                    unsigned short newFatSector = AllocateNewClusterToFile(currentFatSector);

                    // Set the current sector
                    currentFatSector = newFatSector;
                }
                else
                {
                    // Set the current sector
                    currentFatSector = nextFatSector;
                }
            }

            // When the data is stored across the last boundary of the current sector, we have allocate
            // an additional cluster to the file
            if ((offsetWithinCluster + Length >= BYTES_PER_SECTOR) && (descriptor->FileSize < descriptor->CurrentFileOffset + Length))
            {
                // Allocate a new cluster for the file
                AllocateNewClusterToFile(currentFatSector);
            }

            // Calculate the following disk sector
            unsigned short fatSectorFollowing = FATRead(currentFatSector);

            // Read the specific sector from disk
            ReadSectors((unsigned char *)file_buffer, currentFatSector + DATA_AREA_BEGINNING, 1);
            
            // Read the following logical sector, when the data is stored across 2 disk sectors
            if (offsetWithinCluster + Length >= BYTES_PER_SECTOR)
            {
                ReadSectors((unsigned char *)(file_buffer + BYTES_PER_SECTOR), fatSectorFollowing + DATA_AREA_BEGINNING, 1);
            }

            // Copy the requested data into the destination disk sector
            memcpy(file_buffer + offsetWithinCluster, Buffer, Length);
            
            // Write the specific sector to disk
            WriteSectors((unsigned int *)file_buffer, currentFatSector + DATA_AREA_BEGINNING, 1);
            
            // Write the following logical sector, when the data is stored across 2 disk sectors
            if (offsetWithinCluster + Length >= BYTES_PER_SECTOR)
            {
                WriteSectors((unsigned int *)(file_buffer + BYTES_PER_SECTOR), fatSectorFollowing + DATA_AREA_BEGINNING, 1);
            }

            // Release the file buffer
            free(file_buffer);

            // Set the last Access and Write Date
            SetLastAccessDate(entry);

            // Set the current file position within the FileDescriptor
            descriptor->CurrentFileOffset += Length;

            // Check if the file size has changed
            if (descriptor->CurrentFileOffset > entry->FileSize)
            {
                // Change the data in the RootDirectory
                entry->FileSize = descriptor->CurrentFileOffset;
                descriptor->FileSize = descriptor->CurrentFileOffset;
            }

            // Write the RootDirectory and the FAT tables back to disk
            WriteRootDirectoryAndFAT();
        }
    }
}

// Seeks to the specific position in the file
int SeekFile(unsigned long FileHandle, unsigned long NewFileOffset)
{
    // Find the file from which we want to read
    ListEntry *entry = GetEntryFromList(FileDescriptorList, FileHandle);
    FileDescriptor *descriptor = (FileDescriptor *)entry->Payload;

    if (descriptor != 0x0)
    {
        descriptor->CurrentFileOffset = NewFileOffset;
    }

    return 0;
}

// Returns a flag if the file offset within the FileDescriptor has reached the end of file
int EndOfFile(unsigned long FileHandle)
{
    // Find the file from which we want to check the EndOfFile condition
    ListEntry *entry = GetEntryFromList(FileDescriptorList, FileHandle);
    FileDescriptor *descriptor = (FileDescriptor *)entry->Payload;

    if (descriptor->CurrentFileOffset == descriptor->FileSize)
        return 1;
    else
        return 0;
}

// Prints out the FileDescriptorList entries
void PrintFileDescriptorList()
{
    ListEntry *currentEntry = FileDescriptorList->RootEntry;
    FileDescriptor *descriptor = (FileDescriptor *)currentEntry->Payload;
    
    // Iterate over the whole list
    while (currentEntry != 0x0)
    {
        printf("FileName: ");
        printf(descriptor->FileName);
        printf(", Extension: ");
        printf(descriptor->Extension);
        printf("\nCurrentPosition: 0x");
        printf_long(descriptor->CurrentFileOffset, 16);
        printf(", HashValue: ");
        printf_long(currentEntry->Key, 10);
        printf("\n");
    
        // Move to the next entry in the Double Linked List
        currentEntry = currentEntry->Next;
        descriptor = (FileDescriptor *)currentEntry->Payload;

    } 

    printf("\n");
}

// Prints out the FAT12 chain
void PrintFATChain()
{
    unsigned short Cluster = 1;
    unsigned short result = 1;

    // Iterate over each cluster
    for (int i = 0; i < 2880; i++)
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

            if (result > 0)
            {
                printf("Cluster ");
                printf_int(Cluster, 10);
                printf(" => ");
                printf_int(result, 10);
                printf("\n");

                if (result >= EOF)
                    printf("\n");
            }
        }
        else
        {
            // Even Cluster
            result = val & 0x0FFF; // Lowest 12 Bits

            if (result > 0)
            {
                printf("Cluster ");
                printf_int(Cluster, 10);
                printf(" => ");
                printf_int(result, 10);
                printf("\n");

                if (result >= EOF)
                    printf("\n");
            }
        }
    }
}

// Tests some functionality of the FAT12 file system
void FAT12Test()
{
    CreateFile("TEST    ", "TXT", "Das ist ein Test von Klaus");

    unsigned long fileHandle = OpenFile("TEST    ", "TXT");
    SeekFile(fileHandle, 2000);
    WriteFile(fileHandle, "Aschenbrenner", 13);

    SeekFile(fileHandle, 700);
    WriteFile(fileHandle, "Pichlgasse 16/6, 1220 Wien", 26);

    SeekFile(fileHandle, 3000);
    WriteFile(fileHandle, "Karin Hochstöger-Aschenbrenner", 30);
    CloseFile(fileHandle);

    fileHandle = OpenFile("TEST    ", "TXT");
    SeekFile(fileHandle, 1009);
    WriteFile(fileHandle, "Sektoruebergreifendes Schreiben...", 34);
    CloseFile(fileHandle); 
}

// Deallocates the FAT clusters for a file - beginning with the given first cluster
static void DeallocateFATClusters(unsigned short FirstCluster)
{
    // Prepare an empty cluster
    unsigned char *emptyCluster = (unsigned char *)malloc(BYTES_PER_SECTOR);
    memset(emptyCluster, 0x0, BYTES_PER_SECTOR);

    // Read the next cluster of the file
    unsigned short nextCluster = FATRead(FirstCluster);

    // Deallocate the first cluster of the file
    FATWrite(FirstCluster, 0x0);

    // Zero-initialize the old cluster
    WriteSectors((unsigned int *)emptyCluster, FirstCluster + DATA_AREA_BEGINNING, 1);
    
    while (nextCluster < EOF)
    {
        unsigned short currentCluster = nextCluster;

        // Read the next Cluster of the file
        nextCluster = FATRead(nextCluster);

        // Deallocate the current cluster of the file
        FATWrite(currentCluster, 0x0);

        // Zero-initialize the old cluster
        WriteSectors((unsigned int *)emptyCluster, currentCluster + DATA_AREA_BEGINNING, 1);
    }

    // Release the empty cluster
    free(emptyCluster);
}

// Finds the next free Root Directory Entry
static RootDirectoryEntry *FindNextFreeRootDirectoryEntry()
{
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

// Writes the provided value to the specific FAT12 cluster
static void FATWrite(unsigned short Cluster, unsigned short Value)
{
    // Calculate the offset into the FAT table
    unsigned int fatOffset = (Cluster / 2) + Cluster;
   
    if (Cluster % 2 == 0)
    {
        // Even Cluster
        FAT_BUFFER[fatOffset + 0] = (0xff & Value);
        FAT_BUFFER[fatOffset + 1] = ((0xf0 & (FAT_BUFFER[fatOffset + 1])) |  (0x0f & (Value >> 8)));
    }
    else
    {
        // Odd Cluster
        FAT_BUFFER[fatOffset + 0] = ((0x0f & (FAT_BUFFER[fatOffset + 0])) | ((0x0f & Value) << 4));
        FAT_BUFFER[fatOffset + 1] = ((0xff) & (Value >> 4));
    }
}

// Finds the next free FAT12 cluster entry
static unsigned short FindNextFreeFATEntry()
{
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
        }
        else
        {
            // Even Cluster
            result = val & 0x0FFF; // Lowest 12 Bits
        }
    }

    return Cluster;
}

// Load all Clusters for the given Root Directory Entry into memory
static void LoadProgramIntoMemory(RootDirectoryEntry *Entry)
{
    // Read the first cluster of the Kernel into memory
    unsigned char *program_buffer = (unsigned char *)EXECUTABLE_BASE_ADDRESS_PTR;
    ReadSectors((unsigned char *)program_buffer, Entry->FirstCluster + DATA_AREA_BEGINNING, 1);
    unsigned short nextCluster = FATRead(Entry->FirstCluster);

    // Read the whole file into memory until we reach the EOF mark
    while (nextCluster < EOF)
    {
        program_buffer = program_buffer + BYTES_PER_SECTOR;
        ReadSectors(program_buffer, nextCluster + DATA_AREA_BEGINNING, 1);

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
    ReadSectors((unsigned char *)FAT_BUFFER, FAT1_CLUSTER, FAT_COUNT * SECTORS_PER_FAT);
}

// Writes the Root Directory and the FAT12 tables from the memory back to the disk
static void WriteRootDirectoryAndFAT()
{
    // Calculate the Root Directory Size: 14 sectors: => 32 * 224 / 512
    short rootDirectorySectors = 32 * ROOT_DIRECTORY_ENTRIES / BYTES_PER_SECTOR;

    // Calculate the LBA address of the Root Directory: 19: => 2 * 9 + 1
    short lbaAddressRootDirectory = FAT_COUNT * SECTORS_PER_FAT + RESERVED_SECTORS;

    // Write the Root Directory back to disk
    WriteSectors((unsigned int *)ROOT_DIRECTORY_BUFFER, lbaAddressRootDirectory, rootDirectorySectors);
    
    // Write both FAT12 tables back to disk
    WriteSectors((unsigned int *)FAT_BUFFER, FAT1_CLUSTER, SECTORS_PER_FAT);
    WriteSectors((unsigned int *)FAT_BUFFER, FAT2_CLUSTER, SECTORS_PER_FAT);
}

// Sets the last Access Date and the last Write Date for the RootDirectoryEntry
static void SetLastAccessDate(RootDirectoryEntry *Entry)
{
    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

    // Set the Date/Time information of the new file
    Entry->LastWriteYear = Entry->LastAccessYear = bib->Year - FAT12_YEAROFFSET;
    Entry->LastWriteMonth = Entry->LastAccessMonth = bib->Month;
    Entry->LastWriteDay = Entry->LastAccessDay = bib->Day;
    Entry->LastWriteHour = bib->Hour;
    Entry->LastWriteMinute = bib->Minute;
    Entry->LastWriteSecond = bib->Second / 2;
}

// Allocates a new cluster to the given FAT sector
static unsigned short AllocateNewClusterToFile(unsigned short CurrentFATSector)
{
    // Allocate a new cluster for the file
    unsigned short newFatSector = FindNextFreeFATEntry();
    FATWrite(CurrentFATSector, newFatSector);
    FATWrite(newFatSector, 0xFFF);

    // Zero-initialize the new cluster and write it to disk
    unsigned char *emptyContent = (unsigned char *)malloc(BYTES_PER_SECTOR);
    memset(emptyContent, 0x00, BYTES_PER_SECTOR);
    WriteSectors((unsigned int *)emptyContent, newFatSector + DATA_AREA_BEGINNING, 1);

    // Release the block of memory
    free(emptyContent);

    // Return the new allocated FAT sector
    return newFatSector;
}

// Calculates a Hash Value for the given file name
// The hash function is based on this article: https://www.codingninjas.com/studio/library/string-hashing-2425
static unsigned long HashFileName(unsigned char *FileName)
{
    int hash = 0;

    int length = strlen(FileName);

    // To store 'P'.
    int p = 1;

    // For taking modulo.
    int m = 1000000007;

    for (int i = 0; i < length; i++)
    {
        hash += (FileName[i] - 'a') * p;
        hash = hash % m;
        p *= 41;
    }

    return hash;
}