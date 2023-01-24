#include "drivers/screen.h"
#include "drivers/keyboard.h"
#include "common.h"
#include "memory.h"

// Memory Region Type
char *MemoryRegionType[] =
{
    "Available",
    "Reserved",
    "ACPI Reclaim",
    "ACPI NVS Memory"
};

// Initializes the physical Memory Manager
void InitMemoryManager()
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    MemoryRegion *region = (MemoryRegion *)MEMORYMAP_OFFSET;
    int i;

    // Loop over each entry
    for (i = 0; i < bib->MemoryMapEntries; i++)
    {
        if (region[i].Type == 1)
        {
            // Available
            bib->AvailableMemory += region[i].Size;
        }
    }
}

// Prints out the memory map that we have obtained from the BIOS
void PrintMemoryMap()
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    MemoryRegion *region = (MemoryRegion *)MEMORYMAP_OFFSET;
    char str[32] = "";
    int i;
    
    // Print out the header information
    itoa(bib->MemoryMapEntries, 10, str);
    printf(str);
    printf(" Memory Map entries found. Press ENTER for next entry.\n");

    // Loop over each entry
    for (i = 0; i < bib->MemoryMapEntries; i++)
    {
        if (region[i].Type == 1)
        {
            // Available
            SetColor(COLOR_GREEN);
        }
        else
        {
            // Everything else
            SetColor(COLOR_LIGHT_RED);
        }

        // Start
        printf("0x");
        ltoa(region[i].Start, 16, str);
        FormatHexString(str, 10);
        printf(str);

        // End
        printf(" - 0x");
        ltoa(region[i].Start + region[i].Size - 1, 16, str);
        FormatHexString(str, 10);
        printf(str);

        // Size
        printf(" Size: 0x");
        ltoa(region[i].Size, 16, str);
        FormatHexString(str, 9);
        printf(str);

        // Size in KB
        printf(" ");
        ltoa(region[i].Size  / 1024, 10, str);
        printf(str);
        printf(" KB");

        // If possible, print out the available size also in MB
        if (region[i].Size > 1024 * 1024)
        {
            ltoa(region[i].Size / 1024  / 1024, 10, str);
            printf(" = ");
            printf(str);
            printf(" MB");
        }
       
        // Memory Region Type
        printf(" (");
        printf(MemoryRegionType[region[i].Type - 1]);
        printf(")");
        printf("\n");

        // Wait for ENTER
        // scanf(str, 10);
    }

    // Reset the color to white
    SetColor(COLOR_WHITE);

    printf("Available Memory: ");
    ltoa(bib->AvailableMemory / 1024 / 1024 + 1, 10, str);
    printf(str);
    printf(" MB");
}

// Memory Map 4 GB - VMware Fusion
// 0x00 0000 0000 - 0x00 0009 F7FF     Size: 0x00 0009 F800         638 KB              Available           653312
// 0x00 0009 F800 - 0x00 0009 FFFF     Size: 0x00 0000 0800           2 KB              Reserved                    2048
// 0x00 000D C000 - 0x00 000F FFFF     Size: 0x00 0002 4000         144 KB              Reserved                    147456
// 0x00 0010 0000 - 0x00 BFED FFFF     Size: 0x00 BFDE 0000     3143552 KB = 3069 MB    Available           3218997248
// 0x00 BFEE 0000 - 0x00 BFEF EFFF     Size: 0x00 0001 F000         124 KB              ACPI Reclaim                126976
// 0x00 BFEF F000 - 0x00 BFEF FFFF     Size: 0x00 0000 1000           4 KB              ACPI NVS Memory             4096
// 0x00 BFF0 0000 - 0x00 BFFF FFFF     Size: 0x00 0010 0000        1024 KB              Available           1048576
// 0x00 F000 0000 - 0x00 F7FF FFFF     Size: 0x00 0800 0000      131072 KB = 128 MB     Reserved                    134217728
// 0x00 FEC0 0000 - 0x00 FEC0 FFFF     Size: 0x00 0001 0000          64 KB              Reserved                    65536
// 0x00 FEE0 0000 - 0x00 FEE0 0FFF     Size: 0x00 0000 1000           4 KB              Reserved                    4096
// 0x00 FFFE DD00 . 0x00 FFFF FFFF     Size: 0x00 0002 0000         128 KB              Reserved                    131072
// 0x01 0000 0000 - 0x01 3FFF FFFF     Size: 0x00 4000 0000     1048576 KB = 1024 MB    Available           1073741824


// The idea of the Physical Memory Manager is to store in a Descriptor for each
// available memory region which page frames (4K large) are available, or in use (bitmap mask).
// With the reported Memory Map from above, we would need in sum 3 Descriptors.
//
// CAUTION:
// The first free memory region is ignored, because it's below the 1MB mark (< 0x100000) -  
// and we want to be on the safe side.
// Only memory regions above the 1MB mark (>= 0x100000) will be available for the physical
// Memory Manager.
// 
// The page frames which are already allocated to the Kernel - which is also loaded 
// at the 1MB mark by KLDR64.BIN, are just marked as in use (bit is set to 1), when these
// Descriptors are initialized.
// 
// The Descriptors needed by the Physical Memory Manager are stored in memory directly
// after KERNEL.BIN. Therefore, we need to know the size of the loaded Kernel.
// 
// The following memory layout can be used to store the information of the Descriptors.
// Number of Memory Region Descriptors
//      => Region #1 Descriptor (32 bytes long)
//          => Physical Start Address
//          => Number of physical Page Frames available (4096 Bytes large)
//              => 785888 (3218997248 / 4096)
//          => Pointer to the physical Bitmap: PTR1
//      => Region #2 Descriptor (32 bytes long)
//          => Physical Start Address
//          => Number of physical Page Frames available (4096 Bytes large)
//              => 256 (1048576 / 4096)
//          => Pointer to the physical Bitmap: PTR2
//      => Region #3 Descriptor (32 bytes long)
//          => Physical Start Address
//          => Number of physical Page Frames available (4096 Bytes large)
//              => 262144 (1073741824 / 4096)
//          => Pointer to the physical Bitmap: PTR3
// Physical Bitmap for Descriptor #1: PTR1
//      => Size depends on the number of Page Frames managed by this Descriptor
//      => 785888 / 8 bits/byte = 98236 bytes long
// Physical Bitmap for Descriptor #2: PTR2
//      => Size depends on the number of Page Frames managed by this Descriptor
//      => 256 / 8 bits/byte = 32 bytes long
// Physical Bitmap for Descriptor #3: PTR3
//      => Size depends on the number of Page Frames managed by this Descriptor
//      => 262144 / 8 bits/byte = 32768 bytes long