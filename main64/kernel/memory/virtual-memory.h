#ifndef VIRTUAL_MEMORY_H
#define VIRTUAL_MEMORY_H

#define SMALL_PAGE_SIZE 4096
#define PT_ENTRIES 512

// ==========================================================================
// The following macros are used to access the various Page Table structures
// through the recursive Page Table Mapping in the 511th entry of the PML4.
// 
// VA == Virtual Address
// PML4 == Page Map Level 4
// PDP == Page Directory Pointer Table
// PD == Page Directory Table
// PT == Page Table
// ==========================================================================

// 0xFFFFFFFFFFFFF000
// Sign Extension   PML4      PDP       PD        PT        Offset
// 1111111111111111 111111111 111111111 111111111 111111111 000000000000
//                  511 dec   511 dec   511 dec   511 dec
//                  => PML4   => PML4   => PML4   => PML4 ==>>> PML4 as a result
#define PML4_TABLE                 ((unsigned long *)(0xFFFFFFFFFFFFF000))

// 0xFFFFFFFFFFE00000
// Sign Extension   PML4      PDP       PD        PT (VA!)  Offset
// 111111111111111 1111111111 111111111 111111111 000000000 000000000000
//                 511 dec    511 dec   511 dec   0 dec
//                 => PML4    => PML4   => PML4   => PDP ==>>> PDP as a result
//
// The correct PDP index entry is taken from the provided virtual memory address
// and is added to the base virtual memory address 0xFFFFFFFFFFE00000:
// 0x00001FF000 =  111111111 000000000000
//                 PT        Offset
#define PDP_TABLE(VirtualAddress) ((unsigned long *)(0xFFFFFFFFFFE00000 + ((((unsigned long)(VirtualAddress)) >> 27) & 0x00001FF000)))

// 0xFFFFFFFFC0000000
// Sign Extension   PML4      PDP       PD (VA!)  PT (VA!)  Offset
// 1111111111111111 111111111 111111111 000000000 000000000 000000000000
//                  511 dec   511 dec   0 dec     0 dec
//                  => PML4   => PML4   => PDP    => PD ==>>> PD as a result
// 
// The correct PDP and PD index entries are taken from the provided virtual memory address
// and are added to the base virtual memory address 0xFFFFFFFFC0000000:
// 0x003FFFF000 =   111111111 111111111 000000000000
//                  PD        PT        Offset
#define PD_TABLE(VirtualAddress) ((unsigned long *)(0xFFFFFFFFC0000000 + ((((unsigned long)(VirtualAddress)) >> 18) & 0x003FFFF000)))

// 0xFFFFFF8000000000
// Sign Extension   PML4      PDP (VA!) PD (VA!)  PT (VA!)  Offset
// 1111111111111111 111111111 000000000 000000000 000000000 000000000000
//                  511 dec   0 dec     0 dec     0 dec
//                  => PML4   => PDP    => PD     => PT ==>>> PT as a result
//
// The correct PDP, PD, and PT index entries are taken from the provided virtual memory address
// and are added to the base virtual memory address 0xFFFFFF8000000000:
// 0x7FFFFFF000 =   111111111 111111111 111111111 000000000000
//                  PDP       PD        PT        Offset
#define PT_TABLE(VirtualAddress) ((unsigned long *)(0xFFFFFF8000000000 + ((((unsigned long)(VirtualAddress)) >> 9)  & 0x7FFFFFF000)))

// ==============================================================================
// The following macros are used to index into the various Page Table structures 
// through the given virtual memory address.
// ==============================================================================

// Macros to index into the various Page Tables
#define PML4_INDEX(VirtualAddress) ((((unsigned long)(VirtualAddress)) >> 39) & PT_ENTRIES - 1)
#define PDP_INDEX(VirtualAddress) ((((unsigned long)(VirtualAddress)) >> 30) & PT_ENTRIES - 1)
#define PD_INDEX(VirtualAddress) ((((unsigned long)(VirtualAddress)) >> 21) & PT_ENTRIES - 1)
#define PT_INDEX(VirtualAddress) ((((unsigned long)(VirtualAddress)) >> 12) & PT_ENTRIES - 1)

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

// Initializes the Paging Data Structures.
void InitVirtualMemoryManager();

// Switches the PML4 Page Table Offset in the CR3 Register.
void SwitchPageDirectory(PageMapLevel4Table *PML4);

// Handles a Page Fault
void HandlePageFault(unsigned long VirtualAddress);

// Tests the Virtual Memory Manager.
void TestVirtualMemoryManager();

static void PageFaultDebugPrint(unsigned long PageTableIndex, char *PageTableName, unsigned long PhysicalFrame);

#endif