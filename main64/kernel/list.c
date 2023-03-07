#include "drivers/screen.h"
#include "memory/heap.h"
#include "list.h"

// Creates a new Double Linked List
List *NewList()
{
    // Allocate a new List structure on the Heap
    List *list = malloc(sizeof(List));
    list->Count = 0;
    list->RootEntry = 0x0;

    return list;
}

// Adds a new entry to the given Double Linked List
void AddEntryToList(List *List, void *Payload, unsigned long Key)
{
    ListEntry *newEntry = NewListEntry(Payload, Key);

    if (!List->RootEntry)
    {
        // Add the first, initial entry to the List
        List->RootEntry = newEntry;
    }
    else
    {
        ListEntry *currentEntry = List->RootEntry;
        
        // Move to the end of the List
        while (currentEntry->Next)
            currentEntry = currentEntry->Next;

        // Add the new entry to the end of the List
        currentEntry->Next = newEntry;
        newEntry->Previous = currentEntry;
    }
    
    // Increment the number of List entries
    List->Count++;
}

// Returns an entry from the given Double Linked List
ListEntry *GetEntryFromList(List *List, unsigned long Key)
{
    ListEntry *currentEntry = List->RootEntry;
    int currentIndex;

    while (currentEntry != 0x0)
    {
        // Check if we have found the requested entry in the Double Linked List
        if (currentEntry->Key == Key)
            return currentEntry;

        // Move to the next entry in the Double Linked List
        currentEntry = currentEntry->Next;
    }

    // The List entry was not found
    return (void *)0x0;
}

// Removes a Node from the given Double Linked List
void RemoveEntryFromList(List *List, ListEntry *Entry, int FreeMemory)
{
    ListEntry *nextEntry = 0x0;
    ListEntry *previousEntry = 0x0;

    if (Entry->Previous == 0x0)
    {
        // When we remove the first list entry, we just set the new root node to the 2nd list entry
        List->RootEntry = Entry->Next;
        List->RootEntry->Previous = 0x0;
    }
    else
    {
        // Remove the ListNode from the Double Linked List
        previousEntry = Entry->Previous;
        nextEntry = Entry->Next;
        previousEntry->Next = nextEntry;
        nextEntry->Previous = previousEntry;
    }

    // Decrement the number of List entries
    List->Count--;

    // Release the memory of the ListNode structure on the Heap
    if (FreeMemory)
        free(Entry);
}

// This function prints out the content of the Double Linked List
void PrintList(List *List)
{
    ListEntry *currentEntry = List->RootEntry;

    printf("Number of List entries: ");
    printf_int(List->Count, 10);
    printf("\n\n");

    // Call the custom print function for the Double Linked List
    List->PrintFunctionPtr();
}

// Creates a new ListEntry structure
static ListEntry *NewListEntry(void *Payload, unsigned long Key)
{
    // Allocate a new ListNode structure on the Heap
    ListEntry *entry = malloc(sizeof(ListEntry));
    entry->Previous = 0x0;
    entry->Next = 0x0;
    entry->Payload = Payload;
    entry->Key = Key;

    return entry;
}