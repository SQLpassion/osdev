#include "virtual-memory.h"
#include "physical-memory.h"
#include "../common.h"

// Initializes the Paging Data Structures
void InitVirtualMemoryManager()
{
    int i = 0;

    // =====================================================================
    // Allocate some 4K large pages for the necessary Page Table structures.
    // =====================================================================
    // The physical Pagea Frames from 0 - 2 MB (0x000000 - 0x1FFFFF) are identity mapped by KLDR16.BIN in longmode.asm
    // before we enter the x64 Long Mode.
    // 
    // This means that the following allocations are *NOT* triggering a Page Fault as long as the Physical Memory Manager
    // returns Page Frames in the identity mapped area. (0x000000 - 0x1FFFFF).
    //
    // NOTE: If the Physical Memory Manager returns a physical Page Frame outside of the identity mapped area (>= 0x200000),
    // it would trigger a Page Fault that we can't really handle, because the necessary paging structures are not initialized yet.
    // This could happen if the Kernel gets larger and larger, and consumes more and more memory in the identity mapped area.
    // In that case the identity mapped area must be enlarged by another 2 MB by KLDR16.BIN.

    PageMapLevel4Table *pml4 = (PageMapLevel4Table *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryPointerTable *pdp = (PageDirectoryPointerTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryTable *pd = (PageDirectoryTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageTable *pt1 = (PageTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageTable *pt2 = (PageTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryPointerTable *pdpIdentityMapped = (PageDirectoryPointerTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageDirectoryTable *pdIdentityMapped = (PageDirectoryTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    PageTable *ptIdentityMapped = (PageTable *)(AllocatePageFrame() * SMALL_PAGE_SIZE);
    
    // Zero initialize the allocated 4K pages
    memset(pml4, 0, sizeof(PageMapLevel4Table));
    memset(pdp, 0, sizeof(PageDirectoryPointerTable));
    memset(pd, 0, sizeof(PageDirectoryTable));
    memset(pt1, 0, sizeof(PageTable));
    memset(pt2, 0, sizeof(PageTable));
    memset(pdpIdentityMapped, 0, sizeof(PageDirectoryPointerTable));
    memset(pdIdentityMapped, 0, sizeof(PageDirectoryTable));
    memset(ptIdentityMapped, 0, sizeof(PageTable));

    // Point in the 1st PDP entry to the PD
    pdpIdentityMapped->Entries[0].Frame = (unsigned long)pdIdentityMapped / SMALL_PAGE_SIZE;
    pdpIdentityMapped->Entries[0].Present = 1;
    pdpIdentityMapped->Entries[0].ReadWrite = 1;
    pdpIdentityMapped->Entries[0].User = 1;

    // Point in the 1st PD entry to the PT
    pdIdentityMapped->Entries[0].Frame = (unsigned long)ptIdentityMapped / SMALL_PAGE_SIZE;
    pdIdentityMapped->Entries[0].Present = 1;
    pdIdentityMapped->Entries[0].ReadWrite = 1;
    pdIdentityMapped->Entries[0].User = 1;

    // Identity Mapping of the first 256 small pages of 4K (0 - 1 MB Virtual Address Space)
    // In that area we have all the various I/O ports and the above allocated Page Table Structure
    for (i = 0; i < 256; i++)
    {
        ptIdentityMapped->Entries[i].Frame = i;
        ptIdentityMapped->Entries[i].Present = 1;
        ptIdentityMapped->Entries[i].ReadWrite = 1;
        ptIdentityMapped->Entries[i].User = 1;
    }

    // Identity Mapping of 0 - 1 MB (up to 0x100000 - just below the Kernel), so that the above allocated Page Tables can be still accessed
    // after we have switched the Page Directory
    pml4->Entries[0].Frame = (unsigned long)pdpIdentityMapped / SMALL_PAGE_SIZE;
    pml4->Entries[0].Present = 1;
    pml4->Entries[0].ReadWrite = 1;
    pml4->Entries[0].User = 1;

    // Point in the 1st PML4 entry to the PDP
    pml4->Entries[256].Frame = (unsigned long)pdp / SMALL_PAGE_SIZE;
    pml4->Entries[256].Present = 1;
    pml4->Entries[256].ReadWrite = 1;
    pml4->Entries[256].User = 1;

    // Install the Recursive Page Table Mapping
    pml4->Entries[511].Frame = (unsigned long)pml4 / SMALL_PAGE_SIZE;
    pml4->Entries[511].Present = 1;
    pml4->Entries[511].ReadWrite = 1;
    pml4->Entries[511].User = 1;

    // Point in the 1st PDP entry to the PD
    pdp->Entries[0].Frame = (unsigned long)pd / SMALL_PAGE_SIZE;
    pdp->Entries[0].Present = 1;
    pdp->Entries[0].ReadWrite = 1;
    pdp->Entries[0].User = 1;

    // Point in the 1st PD entry to the PT
    pd->Entries[0].Frame = (unsigned long)pt1 / SMALL_PAGE_SIZE;
    pd->Entries[0].Present = 1;
    pd->Entries[0].ReadWrite = 1;
    pd->Entries[0].User = 1;

    // Mapping of the first 512 small pages of 4K (0 - 2 MB Virtual Address Space)
    // with a base offset of 0xFFFF800000000000
    for (i = 0; i < PT_ENTRIES; i++)
    {
        pt1->Entries[i].Frame = i;
        pt1->Entries[i].Present = 1;
        pt1->Entries[i].ReadWrite = 1;
        pt1->Entries[i].User = 1;
    }

    // Point in the 2nd PD entry to the 2nd PT
    pd->Entries[1].Frame = (unsigned long)pt2 / SMALL_PAGE_SIZE;;
    pd->Entries[1].Present = 1;
    pd->Entries[1].ReadWrite = 1;
    pd->Entries[1].User = 1;

    // Mapping of the next 512 small pages of 4K (2 - 4 MB Virtual Address Space)
    // with a base offset of 0xFFFF800000000000
    for (i = 0; i < PT_ENTRIES; i++)
    {
        pt2->Entries[i].Frame = i + (PT_ENTRIES * 1);
        pt2->Entries[i].Present = 1;
        pt2->Entries[i].ReadWrite = 1;
        pt2->Entries[i].User = 1;
    }

    // Stores the Memory Address of PML4 in the CR3 register
    SwitchPageDirectory(pml4);
}

// Switches the PML4 Page Table Offset in the CR3 Register
void SwitchPageDirectory(PageMapLevel4Table *PML4)
{
    asm volatile("mov %0, %%cr3":: "r"(PML4));
}