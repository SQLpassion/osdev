#ifndef PHYSICAL_MEMORY_H
#define PHYSICAL_MEMORY_H

// The offset where the Memory Map is stored
#define MEMORYMAP_OFFSET 0x1200

#define PAGE_SIZE 4096
#define BITS_PER_BYTE 8
#define MARK_1MB 0x100000

#define INDEX_FROM_BIT(a) (a / ( 8 * 4 * 2))
#define OFFSET_FROM_BIT(a) (a % ( 8 * 4 * 2))

// Describes a Memory Map Entry that we have obtained from the BIOS.
typedef struct BiosMemoryRegion
{
    unsigned long Start;    // Physical Start Address
    unsigned long Size;     // Length in Bytes
    int	Type;               // Type - see MemoryRegionType below
    int	Reserved;           // Reserved
} BiosMemoryRegion;

// Describes a single Memory Region that is managed by the Physical Memory Manager.
typedef struct PhysicalMemoryRegionDescriptor
{
    unsigned long PhysicalMemoryStartAddress;   // Physical memory address, where the memory region starts
    unsigned long AvailablePageFrames;          // The number of physical Page Frames that are available
    unsigned long BitmapMaskStartAddress;       // Physical memory address, where the bitmap mask is stored
    unsigned long BitmapMaskSize;               // The size of the bitmap mask in bytes
    unsigned long FreePageFrames;               // The number of free (unallocated) Page Frames
} PhysicalMemoryRegionDescriptor;

// Describes the whole memory layout that is managed by the
// Physical Memory Manager.
typedef struct PhysicalMemoryLayout
{
    // The number of managed memory regions.
    unsigned int MemoryRegionCount;

    // We have to make sure that we pad all previous structure members to multiple
    // of 8 bytes, otherwise we get an unaligned pointer for the following array.
    // Therefore, we add here 4 additional bytes, so that the array "Regions" starts
    // at a multiple of 8 bytes.
    unsigned int padding;

    // The PhysicalMemoryRegionDescriptor array is at the end of this struct, because
    // it has a dynamic size based on the number of memory regions.
    PhysicalMemoryRegionDescriptor MemoryRegions[];
} PhysicalMemoryLayout;

// Initializes the physical Memory Manager.
void InitPhysicalMemoryManager(int KernelSize);

// Allocates the first free Page Frame and returns the Page Frame number.
unsigned long AllocatePageFrame();

// Prints out the memory map that we have obtained from the BIOS
void PrintMemoryMap();

// Returns the number of used physical Page Frames by the Kernel and the Physical Memory Manager itself
static int GetUsedPageFrames(PhysicalMemoryLayout *MemoryLayout);

// Tests the Bitmap mask functionality
void TestBitmapMask();

// Tests the Physical Memory Manager by allocating Page Frames in the various
// available memory regions...
void TestPhysicalMemoryManager();

#endif