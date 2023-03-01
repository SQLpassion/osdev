#include "heap.h"
#include "../common.h"
#include "../drivers/screen.h"
#include "../drivers/keyboard.h"

unsigned long HEAP_START_OFFSET = 0xFFFF800000500000;
unsigned long HEAP_END_OFFSET =   0xFFFF800000500000;
unsigned long INITIAL_HEAP_SIZE = 0x1000;
unsigned long HEAP_GROWTH =       0x1000;

// Initializes the Heap Manager
int InitHeap()
{
    // Initially the whole Heap is unallocated
    HeapBlock *heap = (HeapBlock *)HEAP_START_OFFSET;
    heap->InUse = 0;
    heap->Size = INITIAL_HEAP_SIZE;

    HEAP_END_OFFSET = HEAP_START_OFFSET + INITIAL_HEAP_SIZE;
    
    // Return the size of the whole Heap
    return heap->Size;
}

// Dumps out the status of each Heap Block
void DumpHeap()
{
    HeapBlock *block;
    char str[32] = "";
    int size = 0;
    
    for (block = (HeapBlock *)HEAP_START_OFFSET; block->Size > 0; block = NextHeapBlock(block))
    {
        size += block->Size;
        PrintHeapBlock(block);
    }

    printf("Heap Start Offset: 0x");
    printf_long(HEAP_START_OFFSET, 16);
    printf("\n");
    printf("Heap End Offset:   0x");
    printf_long(HEAP_END_OFFSET, 16);
    printf("\n");
    printf("Whole Heap Size: ");
    itoa(size, 10, str);
    printf(str);
    printf("\n");
    printf("\n");
}

// Performs an allocation on the Heap.
// The memory allocation must happen single threaded, because otherwise we could corrupt the Heap Data Structure...
void *malloc(int Size)
{
    // Add the size of the Header to the requested size
    Size = Size + HEADER_SIZE;

    // Adjust the size to a 4-byte boundary, so that we can use
    // the lower 2 bits for storing status information
    Size = (Size + HEADER_SIZE - 1) & ~(HEADER_SIZE - 1);

    // Find a free block
    HeapBlock *block = Find(Size);

    if (block != 0)
    {
        // Allocate the free Heap Block
        Allocate(block, Size);

        // Return the address of the payload of the found Heap Block
        return (void *)block->Payload;
    }
    else
    {
        // We don't have found any free Heap Block.
        // Let's allocate another 4K page for the Heap
        HeapBlock *lastBlock = GetLastHeapBlock();
        lastBlock->InUse = 0;
        lastBlock->Size = HEAP_GROWTH;
        HEAP_END_OFFSET += HEAP_GROWTH;

        // Merge the last free block with the newly allocated block together
        Merge();

        // Try to allocate the requested block after the expansion of the Heap...
        return malloc(Size - HEADER_SIZE);
    }
}

// Frees up a Heap Block
void free(void *ptr)
{
    // Mark the Heap Block as Free
    HeapBlock *block = ptr - HEADER_SIZE;
    block->InUse = 0;

    // Merge free blocks together
    int mergedBlocks = Merge();
    
    if (mergedBlocks > 0)
    {
        // If we have merged some free blocks together, we try it again
        // mergedBlocks = Merge();
    }
}

// Tests the Heap Manager
void TestHeapManager()
{
    char input[100] = "";

    void *ptr1 = malloc(100);
    void *ptr2 = malloc(100);
    printf("After malloc():\n");
    DumpHeap();
    scanf(input, 98);

	free(ptr1);
    printf("After free():\n");
	DumpHeap();
    scanf(input, 98);

    void *ptr3 = malloc(50);
    printf("After malloc():\n");
    DumpHeap();
    scanf(input, 98);

    void *ptr4 = malloc(44);
    printf("After malloc():\n");
    DumpHeap();
    scanf(input, 98);

    free(ptr2);
    free(ptr3);
    free(ptr4);
    printf("After free():\n");
	DumpHeap();
}

// Finds a free block of the requested size on the Heap
static HeapBlock *Find(int Size)
{
    HeapBlock *block;

    // Iterate over the various Heap Blocks
    for (block = (HeapBlock *)HEAP_START_OFFSET; block->Size > 0; block = NextHeapBlock(block))
    {
        // Check if we have found a free and large enough Heap Block
        if ((block->InUse == 0) && (Size <= block->Size))
            return block;
    }

    // No free Heap Block was found
    return 0;
}

// Returns the next Heap Block
static HeapBlock *NextHeapBlock(HeapBlock *Block)
{
    // Return a pointer to the next Heap Block
    return (HeapBlock *)((unsigned char *)Block + Block->Size);
}

// Returns the last Heap Block
static HeapBlock *GetLastHeapBlock()
{
    HeapBlock *block = (HeapBlock *)HEAP_START_OFFSET;

    // Loop until we reach the end of the current Heap...
    while (block->Size > 0) block = NextHeapBlock(block);

    // Return the last Heap Block
    return block;
}

// Allocates a Heap Block at the beginning of "*Block" with a size of "Size".
// Splits the remaining available Heap Space and marks it as a free Heap Block
static void Allocate(HeapBlock *Block, int Size)
{
    int oldSize = Block->Size;

    // Check if there is free space available for an additional Heap Block after we have allocated the requested size
    // The minimum remaining size must be the Header Size + 1 Byte payload
    if (Block->Size - Size >= HEADER_SIZE + 1)
    {
        // Resize the current Heap Block
        Block->InUse = 1;
        Block->Size = Size;

        // Split the current Heap Block, because there is a remaining free space after the previous resizing
        HeapBlock *nextBlock = NextHeapBlock(Block);
        nextBlock->InUse = 0;
        nextBlock->Size = oldSize - Size;
    }
    else
    {
        // We don't have to split the current Heap Block, because there is no remaining free space after the Allocation
        // The remaining space is also allocated to this Heap Block!
        // Therefore the Heap Block Size is larger than the requested size!
        Block->InUse = 1;
    }
}

// Merges 2 free blocks into one larger free block
static int Merge()
{
    HeapBlock *block = (HeapBlock *)HEAP_START_OFFSET;
    int mergedBlocks = 0;

    // Iterate over the various Heap Blocks
    while (block->Size > 0)
    {
        HeapBlock *nextBlock = NextHeapBlock(block);

        // If the current and the next block are free, merge them together
        if (block->InUse == 0 && nextBlock->InUse == 0)
        {
            // Merge with the next free Heap Block
            block->Size = block->Size + nextBlock->Size;
            mergedBlocks++;
        }

        block = NextHeapBlock(block);
    }

    // Return the number of merged blocks
    return mergedBlocks;
}

// Dumps out the status of a Heap Block
static void PrintHeapBlock(HeapBlock *Block)
{
    char str[32] = "";
    printf("Heap Block Address: 0x");
    ltoa((unsigned long)Block, 16, str);
    printf(str);
    printf("\n");
    printf("Heap Block Size: ");
    itoa(Block->Size, 10, str);
    printf(str);
    printf("\n");
    printf("Heap Block Status: ");

    if (Block->InUse == 0)
    {
        int color = SetColor(COLOR_LIGHT_GREEN);
        printf("FREE\n\n");
        SetColor(color);
    }
    else
    {
        int color = SetColor(COLOR_LIGHT_RED);
        printf("ALLOCATED\n\n");
        SetColor(color);
    }
}