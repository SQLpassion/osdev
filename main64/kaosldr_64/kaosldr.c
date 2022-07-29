#include "fat12.h"

// Implemented in assembly, jumps to 0x100000 where the Kernel was loaded
extern void ExecuteKernel();

// Entry point of KLDR64.BIN
// The only task of the KLDR64.BIN file is to load the KERNEL.BIN file to the physical 
// memory address 0x100000 and execute it from there.
//
// That task must be done in KLDR64.BIN, because the CPU is now already in x64 Long Mode,
// and therefore we can access higher memory addresses like 0x100000.
// This would be not possible in KLDR16.BIN, because the CPU is here still in x16 Real Mode.
void kaosldr_main()
{
    // Load the x64 OS Kernel into memory for its execution...
    LoadKernelIntoMemory("KERNEL  BIN");

    // Execute the Kernel.
    // This function call will never return...
    ExecuteKernel();

    // This code block will never be reached - it's just there for safety
    while (1 == 1) {}
}