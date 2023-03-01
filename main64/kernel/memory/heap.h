#ifndef HEAP_H
#define HEAP_H

#define HEADER_SIZE 4

typedef struct HeapBlock
{
    // Header: 4 bytes
    int InUse : 1;
    int Size : 31;

    // Payload
    unsigned char Payload[0];
} HeapBlock;

// Initializes the Heap Manager
int InitHeap();

// Dumps out the status of each Heap Block
void DumpHeap();

// Allocates the specific amount of memory on the Heap
void *malloc(int Size);

// Frees up a Heap Block
void free(void *ptr);

// Tests the Heap Manager with simple malloc()/free() calls.
void TestHeapManager(int DebugOutput);

// Tests the Heap Manaager across Page boundaries.
void TestHeapManagerAcrossPageBoundaries(int DebugOutput);

// Tests the Heap Manager with huge allocation requests.
void TestHeapManagerWithHugeAllocations(int DebugOutput);

// Finds a free block of the requested size on the Heap
static HeapBlock *Find(int Size);

// Returns the next Heap Block
static HeapBlock *NextHeapBlock(HeapBlock *Block);

// Returns the last Heap Block
static HeapBlock *GetLastHeapBlock();

// Allocates a Heap Block at the beginning of "*Block" with a size of "Size".
// Splits the remaining available Heap Space and marks it as a free Heap Block
static void Allocate(HeapBlock *Block, int Size);

// Merges 2 free blocks into one larger free block
static int Merge();

// Dumps out the status of a Heap Block
static void PrintHeapBlock(HeapBlock *Block);

#endif