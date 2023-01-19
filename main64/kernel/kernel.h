#ifndef KERNEL_H
#define KERNEL_H

typedef struct MemoryRegion
{
	unsigned long Start;
	unsigned long Size;
	int	Type;
	int	Reserved;
} MemoryRegion;

char *MemoryRegionType[] =
{
	"Available",
	"Reserved",
	"ACPI Reclaim",
	"ACPI NVS Memory"
};

// The main entry of our Kernel
void KernelMain();

// Initializes the whole Kernel
void InitKernel();

// Causes a Divide by Zero Exception
void DivideByZeroException();

// Tests the functionality of the keyboard
void KeyboardTest();

// Dumps out the Memory Map
void DumpMemoryMap();

#endif