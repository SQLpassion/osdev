#include "gdt.h"
#include "../common.h"
#include "../memory/heap.h"

// The needed GDT and TSS structures
GdtPointer gdtPointer;
GdtEntry *gdtEntries = (GdtEntry *)GDT_START_OFFSET;
TssEntry *tssEntry = (TssEntry *)TSS_START_OFFSET;

// Installs the various need GDT entries
void InitGdt()
{
    // Initialize the GDT
    gdtPointer.Limit = sizeof(GdtEntry) * (GDT_ENTRIES + 1);
    gdtPointer.Base = (unsigned long)gdtEntries;
    memset(gdtEntries, 0, sizeof(GdtEntry) * (GDT_ENTRIES + 1));

    // Initialize the TSS
    memset(tssEntry, 0, sizeof(tssEntry));

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
    GdtSetGate(5, (unsigned long)tssEntry, sizeof(TssEntry), 0x89, 0x40);

    // Install the new GDT
    GdtFlush((unsigned long)&gdtPointer);
}

// Returns the TSS entry
TssEntry *GetTss()
{
    return tssEntry;
}

// Sets the GDT entry
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