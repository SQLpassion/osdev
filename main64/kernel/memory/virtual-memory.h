#ifndef VIRTUAL_MEMORY_H
#define VIRTUAL_MEMORY_H

#define SMALL_PAGE_SIZE 4096
#define PT_ENTRIES 512

// Represents a 64-bit long Page Map Level 4 Entry
struct PML4Entry
{
    unsigned Present : 1;           // P
    unsigned ReadWrite : 1;         // R/W
    unsigned User : 1;              // U/S
    unsigned WriteThrough : 1;      // PWT
    unsigned CacheDisable : 1;      // PCD
    unsigned Accessed : 1;          // A
    unsigned Ignored1 : 1;          // IGN
    unsigned PageSize : 1;          
    unsigned Ignored2 : 4;          
    unsigned long Frame : 36;
    unsigned short Reserved;
} __attribute__ ((packed));
typedef struct PML4Entry PML4Entry;

// Represents a 64-bit long Page Directory Pointer Entry
struct PDPEntry
{
    unsigned Present : 1;           // P
    unsigned ReadWrite : 1;         // R/W
    unsigned User : 1;              // U/S
    unsigned WriteThrough : 1;      // PWT
    unsigned CacheDisable : 1;      // PCD
    unsigned Accessed : 1;          // A
    unsigned Ignored1 : 1;          // IGN
    unsigned PageSize : 1;          
    unsigned Ignored2 : 4;          
    unsigned long Frame : 36;
    unsigned short Reserved;
} __attribute__ ((packed));
typedef struct PDPEntry PDPEntry;

// Represents a 64-bit long Page Directory Entry
struct PDEntry
{
    unsigned Present : 1;           // P
    unsigned ReadWrite : 1;         // R/W
    unsigned User : 1;              // U/S
    unsigned WriteThrough : 1;      // PWT
    unsigned CacheDisable : 1;      // PCD
    unsigned Accessed : 1;          // A
    unsigned Ignored1 : 1;          // IGN
    unsigned PageSize : 1;          
    unsigned Ignored2 : 4;          
    unsigned long Frame : 36;
    unsigned short Reserved;
} __attribute__ ((packed));
typedef struct PDEntry PDEntry;

// Represents a 64-bit long Page Table Entry
struct PTEntry
{
    unsigned Present : 1;       // P
    unsigned ReadWrite: 1;      // R/W
    unsigned User : 1;          // U/S
    unsigned WriteThrough : 1;  // PWT
    unsigned CacheDisable : 1;  // PCD
    unsigned Accessed : 1;      // A
    unsigned Dirty : 1;         // D
    unsigned PageSize : 1;      // PS
    unsigned Global : 1;        // G
    unsigned Available : 3;     // AVL
    unsigned long Frame : 36;
    unsigned short Reserved;    // 16 Bits
} __attribute__ ((packed));
typedef struct PTEntry PTEntry;

// Defines the Page Map Level 4 Table
typedef struct PageMapLevel4Table
{
    PML4Entry Entries[512];
} PageMapLevel4Table;

// Defines the Page Directory Pointer Table
typedef struct PageDirectoryPointerTable
{
    PDPEntry Entries[512];
} PageDirectoryPointerTable;

// Defines the Page Directory Table
typedef struct PageDirectoryTable
{
    PDEntry Entries[512];
} PageDirectoryTable;

// Defines the Page Table
typedef struct PageTable
{
    PTEntry Entries[512];
} PageTable;

// Initializes the Paging Data Structures
void InitVirtualMemoryManager();

// Switches the PML4 Page Table Offset in the CR3 Register
void SwitchPageDirectory(PageMapLevel4Table *PML4);

#endif