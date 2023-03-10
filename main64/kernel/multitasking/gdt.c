#include "gdt.h"
#include "../common.h"
#include "../memory/heap.h"

GdtPointer *gdtPointer;
GdtEntry *gdtEntries;

// Installs the various need GDT Entries
void InitGdt()
{
    // Initialize the GDT Table
    gdtEntries = malloc(sizeof(GdtEntry) * (GDT_ENTRIES + 1));
    memset(gdtEntries, 0, sizeof(GdtEntry) * (GDT_ENTRIES + 1));

    // Initialize the GDT Pointer
    gdtPointer = malloc(sizeof(GdtPointer));
    memset(gdtPointer, 0, sizeof(GdtPointer));
    gdtPointer->Limit = sizeof(GdtEntry) * (GDT_ENTRIES + 1) - 1;
    gdtPointer->Base = (unsigned long)gdtEntries;

    // Initialize the TSS entry with the Kernel Mode Stack Pointer (RSP) for the first initial Task
    // tssEntry = malloc(sizeof(TSSEntry));
    // memset(tssEntry, 0, sizeof(TSSEntry));
    // tssEntry->rsp0 = 0xFFFF800001000000;

    // The NULL Descriptor
    GdtSetGate(0, 0, 0, 0, 0);

    // The Code Segment Descriptor for Ring 0
    GdtSetGate(1, 0, 0, GDT_FLAG_RING0 | GDT_FLAG_SEGMENT | GDT_FLAG_CODESEG | GDT_FLAG_PRESENT, GDT_FLAG_64_BIT);

    // The Data Segment Descriptor for Ring 0
    GdtSetGate(2, 0, 0, GDT_FLAG_RING0 | GDT_FLAG_SEGMENT | GDT_FLAG_DATASEG | GDT_FLAG_PRESENT, 0);

    // The Code Segment Descriptor for Ring 3
    GdtSetGate(3, 0, 0, GDT_FLAG_RING3 | GDT_FLAG_SEGMENT | GDT_FLAG_CODESEG | GDT_FLAG_PRESENT, GDT_FLAG_64_BIT);

    // The Data Segment Descriptor for Ring 3
    GdtSetGate(4, 0, 0, GDT_FLAG_RING3 | GDT_FLAG_SEGMENT | GDT_FLAG_DATASEG | GDT_FLAG_PRESENT, 0);

    // The TSS Entry
    // GdtSetGate(5, tssEntry, sizeof(TSSEntry), 0x89, 0x40);

    // Install the new GDT
    GdtFlush((unsigned long)gdtPointer);

    // Store the references to the GDT and TSS in the KPCR data structure
    // KPCR *kpcr = (KPCR *)GetKPCR();
    // kpcr->GDT = gdtEntries;
    // kpcr->TSS = tssEntry;
}

// Sets the GDT Entry
void GdtSetGate(unsigned char Num, unsigned long Base, unsigned long Limit, unsigned char Access, unsigned char Granularity)
{
    gdtEntries[Num].BaseLow = Base & 0xFFFF;
    gdtEntries[Num].BaseMiddle = ((Base >> 16) & 0xFF);
    gdtEntries[Num].BaseHigh = ((Base >> 24) & 0xFF);
    gdtEntries[Num].LimitLow = Limit & 0xFFFF;
    gdtEntries[Num].Granularity = ((Limit >> 16) & 0x0F);
    gdtEntries[Num].Granularity |= (Granularity & 0xF0);
    gdtEntries[Num].Access = Access;
}