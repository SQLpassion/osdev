#ifndef MEMORY_H
#define MEMORY_H

// The offset where the Memory Map is stored
#define MEMORYMAP_OFFSET 0x1200

// Describes a Memory Map Entry that we have
// obtained from the BIOS.
typedef struct MemoryRegion
{
    unsigned long Start;	// Physical Start Address
    unsigned long Size;		// Length in Bytes
    int	Type;				// Type - see MemoryRegionType below
    int	Reserved;			// Reserved
} MemoryRegion;

// Describes a single Memory Region that is managed by the
// Physical Memory Manager.
typedef struct MemoryRegionDescriptor
{
    unsigned long PhysicalMemoryStartAddress;
    unsigned long AvailablePageFrames;
    unsigned long BitmapMaskStartAddress;
} MemoryRegionDescriptor;

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

    // The MemoryRegionDescriptor array is at the end of this struct, because
    // it has a dynamic size based on the number of memory regions.
    MemoryRegionDescriptor Regions[];
} PhysicalMemoryLayout;

// Initializes the physical Memory Manager
void InitMemoryManager();

// Prints out the memory map that we have obtained from the BIOS
void PrintMemoryMap();

#endif