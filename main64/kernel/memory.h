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

// Prints out the memory map that we have obtained from the BIOS
void PrintMemoryMap();

#endif