#include "heap.h"
#include "../common.h"
#include "../drivers/screen.h"
#include "../drivers/keyboard.h"

unsigned long HEAP_START_OFFSET = 0xFFFF800000500000;
unsigned long HEAP_END_OFFSET =   0xFFFF800000500000;
unsigned long INITIAL_HEAP_SIZE = 0x1000;
unsigned long HEAP_GROWTH =       0x1000;

int isHeapInitialized = 0;

// Initializes the Heap Manager
int InitHeap()
{
    // Initially the whole Heap is unallocated
    HeapBlock *heap = (HeapBlock *)HEAP_START_OFFSET;
    HEAP_END_OFFSET = HEAP_START_OFFSET + INITIAL_HEAP_SIZE;
    memset(heap, 0x00, INITIAL_HEAP_SIZE);

    // Initialize the Header of the first Heap Block
    heap->InUse = 0;
    heap->Size = INITIAL_HEAP_SIZE;

    // The Heap Manager is now fully initialized, and can be used by other components
    isHeapInitialized = 1;
    
    // Return the size of the whole Heap
    return heap->Size;
}

// Returns if the Heap Manager is fully initialized
int IsHeapInitialized()
{
    return isHeapInitialized;
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
        scanf(str, 30);
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
        // Let's allocate another 4K page for the Heap by just changing the HEAP_END_OFFSET variable.
        // This will also trigger a Page Fault in the background, which will be handled transparently by allocating another physical Page Frame.
        HeapBlock *lastBlock = GetLastHeapBlock();
        lastBlock->InUse = 0;
        lastBlock->Size = HEAP_GROWTH;
        HEAP_END_OFFSET += HEAP_GROWTH;

        // Merge the last free block with the newly allocated block together
        Merge();

        // Try to allocate the requested block after the expansion of the Heap.
        // If the Heap is still too small after the current expansion, the next recursive malloc() call will again expand
        // the Heap, until we have reached the necessary Heap size.
        return malloc(Size - HEADER_SIZE);
    }
}

// Frees up a Heap Block
void free(void *ptr)
{
    // Mark the Heap Block as Free
    HeapBlock *block = (HeapBlock *)((unsigned char *)ptr - HEADER_SIZE);
    block->InUse = 0;

    // Merge all free blocks together
    while (Merge() > 1) {}
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
    int mergedBlocks = 0;

    // Iterate over the various Heap Blocks
    for (HeapBlock *block = (HeapBlock *)HEAP_START_OFFSET; block->Size > 0; block = NextHeapBlock(block))
    {
        HeapBlock *nextBlock = NextHeapBlock(block);

        // If the current and the next block are free, merge them together
        if ((block->InUse == 0) && (nextBlock->InUse == 0))
        {
            // Merge with the next free Heap Block
            block->Size = block->Size + nextBlock->Size;
           
            mergedBlocks++;
        } 
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

// Tests the Heap Manager with simple malloc()/free() calls
void TestHeapManager(int DebugOutput)
{
    char input[100] = "";

    // 104 bytes are allocated (100 + 4 byte Header)
    void *ptr1 = malloc(100);

    // 104 bytes are allocated (100 + 4 byte Header)
    void *ptr2 = malloc(100);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 104 (*ptr1)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500068
        // Heap Block Size: 104 (*ptr2)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005000D0
        // Heap Block Size: 3888
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release a Heap Block of 104 bytes
    free(ptr1);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 104
        // Heap Block Status: FREE

        // Heap Block Adress: 0xFFFF800000500068
        // Heap Block Size: 104 (*ptr2)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005000D0
        // Heap Block Size: 3888
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // 56 bytes are allocated (52 [adjusted to a 4-byte boundary] + 4 byte Header)
    void *ptr3 = malloc(50);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 56 (*ptr3)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500038
        // Heap Block Size: 48
        // Heap Block Status: FREE

        // Heap Block Adress: 0xFFFF800000500068
        // Heap Block Size: 104 (*ptr2)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005000D0
        // Heap Block Size: 3888
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // 48 bytes are allocated (44 + 4 byte Header)
    void *ptr4 = malloc(44);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 56 (*ptr3)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500038
        // Heap Block Size: 48 (*ptr4)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500068
        // Heap Block Size: 104 (*ptr2)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005000D0
        // Heap Block Size: 3888
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release a Heap Block of 104 bytes
    free(ptr2);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 56 (*ptr3)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500038
        // Heap Block Size: 48 (*ptr4)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005000D0
        // Heap Block Size: 3992
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release a Heap Block of 56 bytes
    free(ptr3);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 56
        // Heap Block Status: FREE

        // Heap Block Adress: 0xFFFF800000500038
        // Heap Block Size: 48 (*ptr4)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005000D0
        // Heap Block Size: 3992
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release the last Heap Block of 48 bytes
    free(ptr4);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 4096
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }
}

// Tests the Heap Manaager across Page boundaries.
void TestHeapManagerAcrossPageBoundaries(int DebugOutput)
{
    char input[100] = "";

    // 2504 bytes are allocated (2500 + 4 byte Header)
    void *ptr1 = malloc(2500);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 2504 (*ptr1)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005009C8
        // Heap Block Size: 1592
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // 2504 bytes are allocated (2500 + 4 byte Header)
    void *ptr2 = malloc(2500);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 2504 (*ptr1)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005009C8
        // Heap Block Size: 2504 (*ptr2)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000501390
        // Heap Block Size: 3184
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release a Heap Block of 2504 bytes
    free(ptr2);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 2504 (*ptr1)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF8000005009C8
        // Heap Block Size: 5688
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release a Heap Block of 2504 bytes
    free(ptr1);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 8192
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }
}

// Tests the Heap Manager with huge allocation requests.
void TestHeapManagerWithHugeAllocations(int DebugOutput)
{
    char input[100] = "";

    // 104 bytes are allocated (100 + 4 byte Header)
    void *ptr1 = malloc(100);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 104 (*ptr1)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500068
        // Heap Block Size: 3992
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // 20004 bytes are allocated (20000 + 4 byte Header)
    void *ptr2 = malloc(20000);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 104 (*ptr1)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000500068
        // Heap Block Size: 20004 (*ptr2)
        // Heap Block Status: ALLOCATED

        // Heap Block Adress: 0xFFFF800000504E8C
        // Heap Block Size: 372
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }

    // Release all Heap Blocks
    free(ptr1);
    free(ptr2);

    if (DebugOutput)
    {
        // Heap Block Adress: 0xFFFF800000500000
        // Heap Block Size: 20480
        // Heap Block Status: FREE
        ClearScreen();
        DumpHeap();
        scanf(input, 98);
    }
}