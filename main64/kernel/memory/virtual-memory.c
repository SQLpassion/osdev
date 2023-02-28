#include "virtual-memory.h"
#include "physical-memory.h"
#include "../drivers/screen.h"
#include "../common.h"

// Initializes the necessary data structures for the 4-level x64 paging.
// The first 2 MB of physical RAM (0x000000 - 0x1FFFFF) are Identity Mapped to 0x000000 - 0x1FFFFF.
// In addition this memory range is also mapped to the virtual address range 0xFFFF800000000000 - 0xFFFF8000001FFFFF,
// where the x64 OS Kernel resides (starting at 0xFFFF800000100000).

// NOTE: *ALL* other virtual memory addresses are currently *NOT* mapped, which means accessing them triggers a Page Fault.
// The Page Fault Handler will resolve the triggered Page Fault by allocating a new physical Page Frame
// (somewhere physically after the the end of the Kernel), and adding the necessary entries into the Paging Data Structures.

// Access to the physical Paging Structures in the Page Fault Handler happens through a Recursive Page Table Mapping which
// is installed in the last 511th entry of the PML4 data structure.
void InitVirtualMemoryManager()
{
    // =====================================================================
    // Allocate some 4K large pages for the necessary Page Table structures.
    // =====================================================================
    // The physical Pagea Frames from 0 - 2 MB (0x000000 - 0x1FFFFF) are identity mapped by KLDR16.BIN in longmode.asm
    // before we enter the x64 Long Mode.
    // 
    // This means that the following allocations are *NOT* triggering a Page Fault as long as the Physical Memory Manager
    // returns Page Frames in the identity mapped area. (0x000000 - 0x1FFFFF).
    //
    // NOTE: If the Physical Memory Manager returns in the initialization code a physical Page Frame outside of the
    // identity mapped area (>= 0x200000), it would trigger a Page Fault that we can't really handle, because the
    // necessary paging structures are not initialized yet.
    // This could happen if the Kernel gets larger and larger, and consumes more and more memory in the identity mapped area.
    // In that case, the identity mapped area must be enlarged by another 2 MB by KLDR16.BIN.
    
    // The following 4K pages are necessary for the inital 4-level x64 paging structure
    PageMapLevel4Table *pml4 = (PageMapLevel4Table *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryPointerTable *pdpHigherHalfKernel = (PageDirectoryPointerTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryTable *pdHigherHalfKernel = (PageDirectoryTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageTable *pt1HigherHalfKernel = (PageTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryPointerTable *pdpIdentityMapped = (PageDirectoryPointerTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryTable *pdIdentityMapped = (PageDirectoryTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageTable *ptIdentityMapped = (PageTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    int i = 0;
    
    // Zero initialize the allocated 4K pages
    memset(pml4, 0, sizeof(PageMapLevel4Table));
    memset(pdpHigherHalfKernel, 0, sizeof(PageDirectoryPointerTable));
    memset(pdHigherHalfKernel, 0, sizeof(PageDirectoryTable));
    memset(pt1HigherHalfKernel, 0, sizeof(PageTable));
    memset(pdpIdentityMapped, 0, sizeof(PageDirectoryPointerTable));
    memset(pdIdentityMapped, 0, sizeof(PageDirectoryTable));
    memset(ptIdentityMapped, 0, sizeof(PageTable));

    // Point in the 1st PML4 entry to the PDP of the Identity Mapping
    pml4->Entries[0].Frame = (unsigned long)pdpIdentityMapped / SMALL_PAGE_SIZE;
    pml4->Entries[0].Present = 1;
    pml4->Entries[0].ReadWrite = 1;
    pml4->Entries[0].User = 1;

    // Point in the 256th PML4 entry to the PDP of the Higher Half Kernel mapping
    pml4->Entries[256].Frame = (unsigned long)pdpHigherHalfKernel / SMALL_PAGE_SIZE;
    pml4->Entries[256].Present = 1;
    pml4->Entries[256].ReadWrite = 1;
    pml4->Entries[256].User = 1;

    // Install the Recursive Page Table Mapping in the 511th PML4 entry
    pml4->Entries[511].Frame = (unsigned long)pml4 / SMALL_PAGE_SIZE;
    pml4->Entries[511].Present = 1;
    pml4->Entries[511].ReadWrite = 1;
    pml4->Entries[511].User = 1;

    // Point in the 1st PDP entry to the PD of the Identity Mapping
    pdpIdentityMapped->Entries[0].Frame = (unsigned long)pdIdentityMapped / SMALL_PAGE_SIZE;
    pdpIdentityMapped->Entries[0].Present = 1;
    pdpIdentityMapped->Entries[0].ReadWrite = 1;
    pdpIdentityMapped->Entries[0].User = 1;

    // Point in the 1st PD entry to the PT of the Identity Mapping
    pdIdentityMapped->Entries[0].Frame = (unsigned long)ptIdentityMapped / SMALL_PAGE_SIZE;
    pdIdentityMapped->Entries[0].Present = 1;
    pdIdentityMapped->Entries[0].ReadWrite = 1;
    pdIdentityMapped->Entries[0].User = 1;

    // Identity Mapping of the first 512 small pages of 4K (0 - 2 MB Virtual Address Space)
    // In that area we have all the various I/O ports and the above allocated Page Table Structure
    for (i = 0; i < PT_ENTRIES; i++)
    {
        ptIdentityMapped->Entries[i].Frame = i;
        ptIdentityMapped->Entries[i].Present = 1;
        ptIdentityMapped->Entries[i].ReadWrite = 1;
        ptIdentityMapped->Entries[i].User = 1;
    }

    // Point in the 1st PDP entry to the PD of the Higher Half Kernel mapping
    pdpHigherHalfKernel->Entries[0].Frame = (unsigned long)pdHigherHalfKernel / SMALL_PAGE_SIZE;
    pdpHigherHalfKernel->Entries[0].Present = 1;
    pdpHigherHalfKernel->Entries[0].ReadWrite = 1;
    pdpHigherHalfKernel->Entries[0].User = 1;

    // Point in the 1st PD entry to the PT of the Higher Half Kernel mapping
    pdHigherHalfKernel->Entries[0].Frame = (unsigned long)pt1HigherHalfKernel / SMALL_PAGE_SIZE;
    pdHigherHalfKernel->Entries[0].Present = 1;
    pdHigherHalfKernel->Entries[0].ReadWrite = 1;
    pdHigherHalfKernel->Entries[0].User = 1;

    // Mapping of the first 512 small pages of 4K (0 - 2 MB Virtual Address Space)
    // with a base offset of 0xFFFF800000000000
    for (i = 0; i < PT_ENTRIES; i++)
    {
        pt1HigherHalfKernel->Entries[i].Frame = i;
        pt1HigherHalfKernel->Entries[i].Present = 1;
        pt1HigherHalfKernel->Entries[i].ReadWrite = 1;
        pt1HigherHalfKernel->Entries[i].User = 1;
    }

    // Store the memory address of the newly created PML4 data structure in the CR3 register.
    // This switches the Paging data structures to the current ones, and "forgets" the temporary
    // Paging data structures that we have created in KLDR16.BIN.
    SwitchPageDirectory(pml4);
}

// Switches the PML4 Page Table Offset in the CR3 Register
void SwitchPageDirectory(PageMapLevel4Table *PML4)
{
    asm volatile("mov %0, %%cr3":: "r"(PML4));
}

// Handles a Page Fault.
// The function allocates a new physical Page Frame with the Physical Memory Manager,
// and adds the necessary entries in the Paging data structures.
// The Paging Tables are accessed through the Recursive Page Table Mapping technique.
void HandlePageFault(unsigned long VirtualAddress)
{
    // Get references to the various Page Tables through the Recursive Page Table Mapping
    PageMapLevel4Table *pml4 = (PageMapLevel4Table *)PML4_TABLE;
    PageDirectoryPointerTable *pdp = (PageDirectoryPointerTable *)PDP_TABLE(VirtualAddress);
    PageDirectoryTable *pd = (PageDirectoryTable *)PD_TABLE(VirtualAddress);
    PageTable *pt = (PageTable *)PT_TABLE(VirtualAddress);
    char str[32] = "";

    // Set the screen text color to Green
    int color = SetColor(COLOR_GREEN);

    // Debugging Output
    ltoa(VirtualAddress, 16, str);
    printf("Page Fault at virtual address 0x");
    printf(str);
    printf("\n");

    if (pml4->Entries[PML4_INDEX(VirtualAddress)].Present == 0)
    {
        // Allocate a physical frame for the missing PML4 entry
        pml4->Entries[PML4_INDEX(VirtualAddress)].Frame = AllocatePageFrame();
        pml4->Entries[PML4_INDEX(VirtualAddress)].Present = 1;
        pml4->Entries[PML4_INDEX(VirtualAddress)].ReadWrite = 1;
        pml4->Entries[PML4_INDEX(VirtualAddress)].User = 1;

        // Debugging Output
        PageFaultDebugPrint(PML4_INDEX(VirtualAddress), "PML4", pml4->Entries[PML4_INDEX(VirtualAddress)].Frame);
    }

    if (pdp->Entries[PDP_INDEX(VirtualAddress)].Present == 0)
    {
        // Allocate a physical frame for the missing PDP entry
        pdp->Entries[PDP_INDEX(VirtualAddress)].Frame = AllocatePageFrame();
        pdp->Entries[PDP_INDEX(VirtualAddress)].Present = 1;
        pdp->Entries[PDP_INDEX(VirtualAddress)].ReadWrite = 1;
        pdp->Entries[PDP_INDEX(VirtualAddress)].User = 1;

        // Debugging Output
        PageFaultDebugPrint(PDP_INDEX(VirtualAddress), "PDP", pdp->Entries[PDP_INDEX(VirtualAddress)].Frame);
    }

    if (pd->Entries[PD_INDEX(VirtualAddress)].Present == 0)
    {
        // Allocate a physical frame for the missing PD entry
        pd->Entries[PD_INDEX(VirtualAddress)].Frame = AllocatePageFrame();
        pd->Entries[PD_INDEX(VirtualAddress)].Present = 1;
        pd->Entries[PD_INDEX(VirtualAddress)].ReadWrite = 1;
        pd->Entries[PD_INDEX(VirtualAddress)].User = 1;

        // Debugging Output
        PageFaultDebugPrint(PD_INDEX(VirtualAddress), "PD", pd->Entries[PD_INDEX(VirtualAddress)].Frame);
    }

    if (pt->Entries[PT_INDEX(VirtualAddress)].Present == 0)
    {
        // Allocate a physical frame for the missing PT entry
        pt->Entries[PT_INDEX(VirtualAddress)].Frame = AllocatePageFrame();
        pt->Entries[PT_INDEX(VirtualAddress)].Present = 1;
        pt->Entries[PT_INDEX(VirtualAddress)].ReadWrite = 1;
        pt->Entries[PT_INDEX(VirtualAddress)].User = 1;

        // Debugging Output
        PageFaultDebugPrint(PT_INDEX(VirtualAddress), "PT", pt->Entries[PT_INDEX(VirtualAddress)].Frame);
    }

    printf("\n");

    // Reset the screen tet color
    SetColor(color);
}

static void PageFaultDebugPrint(unsigned long PageTableIndex, char *PageTableName, unsigned long PhysicalFrame)
{
    char str[32] = "";

    // Log the Page Fault to the Console Window
    ltoa(PhysicalFrame, 16, str);
    printf("Allocated the physical Page Frame 0x" );
    printf(str);
    printf(" for the ");
    printf(PageTableName);
    printf(" entry 0x");
    ltoa(PageTableIndex, 16, str);
    printf(str);
    printf("\n");
}

// Tests the Virtual Memory Manager.
void TestVirtualMemoryManager()
{
    char *ptr1 = (char *)0xFFFF8000001FFFFF;
    ptr1[1] = 'A';

    char *ptr2 = (char *)0xFFFF800000201000;
    ptr2[0] = 'A';

    char *ptr3 = (char *)0xFFFF8FFFFF201000;
    ptr3[0] = 'A';
}