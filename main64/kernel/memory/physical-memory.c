#include "../drivers/screen.h"
#include "../drivers/keyboard.h"
#include "../common.h"
#include "../list.h"
#include "physical-memory.h"
#include "heap.h"

// Memory Region Type
char *MemoryRegionType[] =
{
    "Available",
    "Reserved",
    "ACPI Reclaim",
    "ACPI NVS Memory"
};

// This list stores the Page Frames that are currently tracked by the Kernel
List *TrackedPageFrames = 0x0;

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
// available memory region which page frames (4K large) are available, or which ones are in use (bitmap mask).
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
// The following memory layout is used to store the information of the Descriptors.
// Number of Memory Region Descriptors
//      => Region #1 Descriptor (40 bytes long)
//          => Physical Start Address
//          => Number of physical Page Frames (4096 Bytes large) available
//              => 785888 (3218997248 / 4096)
//          => Pointer to the physical Bitmap: PTR1
//      => Region #2 Descriptor (40 bytes long)
//          => Physical Start Address
//          => Number of physical Page Frames (4096 Bytes large) available
//              => 256 (1048576 / 4096)
//          => Pointer to the physical Bitmap: PTR2
//      => Region #3 Descriptor (40 bytes long)
//          => Physical Start Address
//          => Number of physical Page Frames (4096 Bytes large) available
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
void InitPhysicalMemoryManager(int KernelSize)
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    BiosMemoryRegion *region = (BiosMemoryRegion *)MEMORYMAP_OFFSET;
    char str[32] = "";
    int i;

    // The structure PhysicalMemoryLayout will be placed directly after the
    // file KERNEL.BIN in physical memory - aligned at the next 4K boundary.
    unsigned long startAddress = KERNEL_OFFSET + AlignNumber(KernelSize, PAGE_SIZE);
    PhysicalMemoryLayout *memLayout = (PhysicalMemoryLayout *)startAddress;
    memLayout->MemoryRegionCount = 0;

    // Store the reference to the PhysicalMemoryLayout structure in the BIB
    bib->PhysicalMemoryLayout = memLayout;

    // Loop over each Memory Map entry that we got from the BIOS
    for (i = 0; i < bib->MemoryMapEntries; i++)
    {
        // Check, if we deal with a free memory region
        if (region[i].Type == 1)
        {
            // Calculate the available physical memory
            bib->MaxMemory += region[i].Size;
            bib->AvailablePageFrames += region[i].Size / PAGE_SIZE;

            // To be on the safe side, we ignore all memory regions below the 1MB mark...
            if (region[i].Start >= MARK_1MB)
            {
                // Create a new MemoryRegionDescriptor.
                // Its memory address is 8 bytes after the start address of the PhysicalMemoryLayout structure.
                PhysicalMemoryRegionDescriptor *descriptor = (PhysicalMemoryRegionDescriptor *)((memLayout->MemoryRegionCount *
                    sizeof(PhysicalMemoryRegionDescriptor)) + startAddress + 8);

                descriptor->PhysicalMemoryStartAddress = region[i].Start;
                descriptor->AvailablePageFrames = region[i].Size / PAGE_SIZE;
                descriptor->BitmapMaskSize = region[i].Size / PAGE_SIZE / BITS_PER_BYTE;
                descriptor->FreePageFrames = descriptor->AvailablePageFrames;

                // Store the MemoryRegionDescriptor in the array
                memLayout->MemoryRegions[memLayout->MemoryRegionCount] = *descriptor;

                // Increment the Memory Region count
                memLayout->MemoryRegionCount++;
            }
        }
    }

    // Calculate the memory address for the bitmap mask of the first Memory Region Descriptor.
    // It is stored in memory directly after the last Memory Region Descriptor.
    unsigned long bitmapMaskStartAddress = (memLayout->MemoryRegionCount * sizeof(PhysicalMemoryRegionDescriptor) + startAddress + 8);

    // Iterate over each Memory Region Descriptor and store the calcuated bitmap mask memory address
    for (i = 0; i < memLayout->MemoryRegionCount; i++)
    {
        // Set the memory address of the bitmap mask
        memLayout->MemoryRegions[i].BitmapMaskStartAddress = bitmapMaskStartAddress;

        // Initialize the whole bitmap mask to zero values
        memset((unsigned long *)memLayout->MemoryRegions[i].BitmapMaskStartAddress, 0x00, memLayout->MemoryRegions[i].BitmapMaskSize);
    
        // The next bitmap mask will be stored in memory directly after the current one
        bitmapMaskStartAddress += memLayout->MemoryRegions[i].BitmapMaskSize;
    }

    // Mark/Allocate the Page Frames used by the Kernel and the Physical Memory Manager itself as used
    for (i = 0; i < GetUsedPageFrames(memLayout); i++)
    {
        // The first n Page Frames in the first Bitmap Mask are marked as used.
        // The allocated Page Frames are not tracked by the Kernel, because the Heap is not yet initalized.
        // This is not really a problem, because we will *NEVER* release these Page Frames.
        AllocatePageFrame();
    }
}

// Allocates the first free Page Frame and returns the Page Frame number.
unsigned long AllocatePageFrame()
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    PhysicalMemoryLayout *memLayout = bib->PhysicalMemoryLayout;

    for (int k = 0; k < memLayout->MemoryRegionCount; k++)
    {
        PhysicalMemoryRegionDescriptor *descriptor = &memLayout->MemoryRegions[k];
        unsigned long *bitmapMask = (unsigned long *)descriptor->BitmapMaskStartAddress;
        unsigned long i = 0;

        for (i = 0; i < descriptor->BitmapMaskSize / 8; i++)
        {
            if (bitmapMask[i] != 0xFFFFFFFFFFFFFFFF)
            {
                for (int j = 0; j < 64; j++)
                {
                    // Test each bit
                    unsigned long bit = (unsigned long)1 << j;

                    if (!(bitmapMask[i] & bit))
                    {
                        // Allocate the Page Frame in the bitmap mask
                        unsigned long frame = (i * 64) + j;
                        SetBit(frame, bitmapMask);

                        // Decrement the number of free Page Frames
                        descriptor->FreePageFrames--;
                        bib->AvailablePageFrames--;

                        // Calculate the Page Frame Number
                        unsigned long pfn = (frame + (descriptor->PhysicalMemoryStartAddress / PAGE_SIZE));

                        // Add the allocated Page Frame to the Tracked list
                        AddPageFrameToTrackedList(pfn, k);

                        // Return the Page Frame number
                        return pfn;
                    }
                }
            }
        }
    }

    return -1;
}

// Releases a physical Page Frame.
void ReleasePageFrame(unsigned long PageFrameNumber)
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    PhysicalMemoryLayout *memLayout = bib->PhysicalMemoryLayout;
    ListEntry *entry = GetEntryFromList(TrackedPageFrames, PageFrameNumber);
    PageFrame *frame = (PageFrame *)entry->Payload;

    // Check if the requested Page Frame was found
    if (frame != 0x0)
    {
        // Get a reference to the correct Bitmap Mask
        PhysicalMemoryRegionDescriptor *descriptor = &memLayout->MemoryRegions[frame->MemoryRegionIndex];
        unsigned long *bitmapMask = (unsigned long *)descriptor->BitmapMaskStartAddress;
        PageFrameNumber = PageFrameNumber - (descriptor->PhysicalMemoryStartAddress / PAGE_SIZE);

        // Clear the bit in the Bitmap Mask
        ClearBit(PageFrameNumber, bitmapMask);

        // Remove the Page Frame from the Tracked list
        RemoveEntryFromList(TrackedPageFrames, entry, 1);

        // Release the memory of the PageFrame structure on the Heap
        free(frame);
    }
}

// This function adds the Page Frame to the TrackedPageFrameList
static void AddPageFrameToTrackedList(unsigned long PageFrameNumber, int MemoryRegionIndex)
{
    // Check if the Heap is already initialized.
    // If yes, we will track the allocated Page Frame, so that it can be released at a later point in time
    if (IsHeapInitialized() == 1)
    {
        // Initialize the TrackedPageFrame List, if needed
        if (TrackedPageFrames == 0x0)
            TrackedPageFrames = NewList();

        // Allocate a new PageFrame structure on the Heap
        PageFrame *pageFrame = (PageFrame *)malloc(sizeof(PageFrame));
        pageFrame->PageFrameNumber = PageFrameNumber;
        pageFrame->MemoryRegionIndex = MemoryRegionIndex;

        // Add the Page Frame to the List
        AddEntryToList(TrackedPageFrames, pageFrame, PageFrameNumber);
    }
}

// This function prints out the currently tracked Page Frames.
void PrintTrackedPageFrameList()
{
    PrintList(TrackedPageFrames);
}

// Prints out the memory map that we have obtained from the BIOS
void PrintMemoryMap()
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    BiosMemoryRegion *region = (BiosMemoryRegion *)MEMORYMAP_OFFSET;
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

    printf("Max Memory: ");
    ltoa(bib->MaxMemory / 1024 / 1024 + 1, 10, str);
    printf(str);
    printf(" MB");
}

// Returns the number of used physical Page Frames by the Kernel and the Physical Memory Manager itself
int GetUsedPageFrames(PhysicalMemoryLayout *MemoryLayout)
{
    // Calculate the last physical memory address used by the Physical Memory Manager
    PhysicalMemoryRegionDescriptor *lastMemoryRegion = &MemoryLayout->MemoryRegions[MemoryLayout->MemoryRegionCount - 1];
    unsigned long lastUsedMemoryAddress = lastMemoryRegion->BitmapMaskStartAddress + lastMemoryRegion->BitmapMaskSize;
    unsigned long usedMemory = lastUsedMemoryAddress - KERNEL_OFFSET;
    
    return usedMemory / PAGE_SIZE + 1;;
}

// Tests the Bitmap mask functionality
void TestBitmapMask()
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    PhysicalMemoryLayout *memLayout = bib->PhysicalMemoryLayout;
    unsigned long *address = (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress;
    memset((void *)memLayout->MemoryRegions[0].BitmapMaskStartAddress, 0x00, memLayout->MemoryRegions[0].BitmapMaskSize);
    char str[32] = "";
    int i;

    // Print out the information from the PhysicalMemoryRegionDescriptors
    for (i = 0; i < memLayout->MemoryRegionCount; i++)
    {
        printf("0x");
        ltoa(memLayout->MemoryRegions[i].PhysicalMemoryStartAddress, 16, str);
        printf(str);
        printf("   ");
        ltoa(memLayout->MemoryRegions[i].AvailablePageFrames, 10, str);
        printf(str);
        printf("   ");
        ltoa(memLayout->MemoryRegions[i].BitmapMaskSize, 10, str);
        printf(str);
        printf("   ");
        printf("0x");
        ltoa(memLayout->MemoryRegions[i].BitmapMaskStartAddress, 16, str);
        printf(str);
        printf("\n");
    }

    printf("\n");

    // 1st unsigned long value in the bitmap mask
    SetBit(7, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);
    SetBit(63, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);

    // 2nd unsigned long value in the bitmap mask
    SetBit(64 + 9, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);
    SetBit(64 + 63, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);

    // 3rd unsigned long value in the bitmap mask
    SetBit(64 + 64 + 7, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);
    SetBit(64 + 64 + 63, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);

    printf("The value at address 0x");
    printf_long((unsigned long)address, 16);
    printf(" is: 0x");
    printf_long(*address, 16);
    printf("\n");

    address++;
    printf("The value at address 0x");
    printf_long((unsigned long)address, 16);
    printf(" is: 0x");
    printf_long(*address, 16);
    printf("\n");

    address++;
    printf("The value at address 0x");
    printf_long((unsigned long)address, 16);
    printf(" is: 0x");
    printf_long(*address, 16);
    printf("\n");
    printf("\n");

    // Check if a specific bit is set
    int result = TestBit(64 + 64 + 63, (unsigned long*)memLayout->MemoryRegions[0].BitmapMaskStartAddress);

    printf_int(result, 10);
    printf("\n");
}

// Tests the Physical Memory Manager by allocating Page Frames in the various
// available memory regions...
void TestPhysicalMemoryManager()
{
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;
    PhysicalMemoryLayout *memLayout = bib->PhysicalMemoryLayout;
    char str[32] = "";
    int i;

    // The following Page Frames are allocated in the 1st available memory block
    for (i = 0; i < 785812; i++)
    {
        AllocatePageFrame();
    }

    // This is the last Page Frame allocated in the 1st available memory block
    unsigned long frame = AllocatePageFrame();
    printf("Last Page Frame in 1st memory region: ");
    printf_long(frame, 10);
    printf("\n");

    // These Page Frames are allocated in the 2nd available memory block
    for (i = 0; i < 255; i++)
    {
        AllocatePageFrame();
    }

    // This is the last Page Frame allocated in the 2nd available memory block
    frame = AllocatePageFrame();
    printf("Last Page Frame in 2nd memory region: ");
    printf_long(frame, 10);
    printf("\n");

    // This Page Frame is allocated in the 3rd available memory block
    frame = AllocatePageFrame();
    printf("First Page Frame in 3rd memory region: ");
    printf_long(frame, 10);
    printf("\n");
    printf("\n");

    // Print out the information from the PhysicalMemoryRegionDescriptors
    for (i = 0; i < memLayout->MemoryRegionCount; i++)
    {
        printf("0x");
        ltoa(memLayout->MemoryRegions[i].PhysicalMemoryStartAddress, 16, str);
        printf(str);
        printf("   ");
        ltoa(memLayout->MemoryRegions[i].AvailablePageFrames, 10, str);
        printf(str);
        printf("   ");
        ltoa(memLayout->MemoryRegions[i].BitmapMaskSize, 10, str);
        printf(str);
        printf("   ");
        printf("0x");
        ltoa(memLayout->MemoryRegions[i].BitmapMaskStartAddress, 16, str);
        printf(str);
        printf("   ");
        ltoa(memLayout->MemoryRegions[i].FreePageFrames, 10, str);
        printf(str);
        printf("\n");
    }
}

// Tests the Page Frame Tracking
void TestPageFrameTracking()
{
    char str[32] = "";

    unsigned long pfn1 = AllocatePageFrame();
    unsigned long pfn2 = AllocatePageFrame();
    unsigned long pfn3 = AllocatePageFrame();
    unsigned long pfn4 = AllocatePageFrame();
    unsigned long pfn5 = AllocatePageFrame();
    unsigned long pfn6 = AllocatePageFrame();

    // 6 Page Frames are currently allocated
    ClearScreen();
    PrintTrackedPageFrameList();
    scanf(str, 10);

    // 1 Page Frame is released
    ReleasePageFrame(pfn4);
    ClearScreen();
    PrintTrackedPageFrameList();
    scanf(str, 10);

    // Allocate another Page Frame.
    // The previous released Page Frame Number is reused.
    unsigned long pfn7 = AllocatePageFrame();
    ClearScreen();
    PrintTrackedPageFrameList();
    scanf(str, 10);

    // Allocate another Page Frame
    unsigned long pfn8 = AllocatePageFrame();
    ClearScreen();
    PrintTrackedPageFrameList();
    scanf(str, 10);

    // Release all remaining Page Frames
    ReleasePageFrame(pfn1);
    ReleasePageFrame(pfn2);
    ReleasePageFrame(pfn3);
    ReleasePageFrame(pfn5);
    ReleasePageFrame(pfn6);
    ReleasePageFrame(pfn7);
    ReleasePageFrame(pfn8);
    
    // The Tracked Page Frame List is now empty again
    ClearScreen();
    PrintTrackedPageFrameList();
    scanf(str, 10);
}