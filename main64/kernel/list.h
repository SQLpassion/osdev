#ifndef LIST_H
#define LIST_H

// This structure defines a single entry in a Double Linked List.
typedef struct ListEntry
{
    void *Payload;              // The actual data of the List entry
    unsigned long Key;          // The unique key of the List entry
    struct ListEntry *Next;     // Pointer to the next List entry
    struct ListEntry *Previous; // Pointer to the previous List entry
} ListEntry;

// Defines a simple Doube Linked List
typedef struct List
{
    int Count;
    ListEntry *RootEntry;
} List;

// Creates a new Double Linked List
List *NewList();

// Adds a new entry to the given Double Linked List
void AddEntryToList(List *List, void *Payload, unsigned long Key);

// Returns an entry from the given Double Linked List
ListEntry *GetEntryFromList(List *List, unsigned long Key);

// Removes an entry from the given Double Linked List
void RemoveEntryFromList(List *List, ListEntry *Node);

// This function prints out the content of the Double Linked List
void PrintList(List *List);

// Creates a new ListEntry structure
static ListEntry *NewListEntry(void *Payload, unsigned long Key);

#endif