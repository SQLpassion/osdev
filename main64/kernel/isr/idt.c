#include "idt.h"
#include "../common.h"
#include "../drivers/screen.h"

// The 256 possible Interrupt Gates are stored from 0x98000 to 0x98FFF (4096 Bytes long - each Entry is 16 Bytes)
IdtEntry *idtEntries = (IdtEntry *)IDT_START_OFFSET;

// The pointer that points to the Interrupt Gates
IdtPointer idtPointer;

// Initializes the IDT table
void InitIdt()
{
    idtPointer.Limit = sizeof(IdtEntry) * IDT_ENTRIES - 1;
    idtPointer.Base = (unsigned long)idtEntries;
    memset(idtEntries, 0, sizeof(IdtEntry) * IDT_ENTRIES);

    // Setup the 32 Exception handler - as described in Volume 3A: 6.15
    IdtSetGate(0,  (unsigned long)Isr0,  IDT_TRAP_GATE);        // Divide Error Exception
    IdtSetGate(1,  (unsigned long)Isr1,  IDT_TRAP_GATE);        // Debug Exception
	IdtSetGate(2,  (unsigned long)Isr2,  IDT_TRAP_GATE);        // Non-Maskable Interrupt
	IdtSetGate(3,  (unsigned long)Isr3,  IDT_TRAP_GATE);        // Breakpoint Exception
	IdtSetGate(4,  (unsigned long)Isr4,  IDT_TRAP_GATE);        // Overflow Exception
	IdtSetGate(5,  (unsigned long)Isr5,  IDT_TRAP_GATE);        // Bound Range Exceeded Exception
	IdtSetGate(6,  (unsigned long)Isr6,  IDT_TRAP_GATE);        // Invalid Opcode Exception
	IdtSetGate(7,  (unsigned long)Isr7,  IDT_TRAP_GATE);        // Device Not Available Exception
	IdtSetGate(8,  (unsigned long)Isr8,  IDT_INTERRUPT_GATE);   // Double Fault Exception
	IdtSetGate(9,  (unsigned long)Isr9,  IDT_TRAP_GATE);        // Coprocessor Segment Overrun
	IdtSetGate(10, (unsigned long)Isr10, IDT_INTERRUPT_GATE);   // Invalid TSS Exception
	IdtSetGate(11, (unsigned long)Isr11, IDT_INTERRUPT_GATE);   // Segment Not Present
	IdtSetGate(12, (unsigned long)Isr12, IDT_INTERRUPT_GATE);   // Stack Fault Exception
	IdtSetGate(13, (unsigned long)Isr13, IDT_INTERRUPT_GATE);   // General Protection Exception
	IdtSetGate(14, (unsigned long)Isr14, IDT_INTERRUPT_GATE);   // Page Fault Exception
	IdtSetGate(15, (unsigned long)Isr15, IDT_TRAP_GATE);        // Unassigned!
	IdtSetGate(16, (unsigned long)Isr16, IDT_TRAP_GATE);        // x87 FPU Floating Point Error
	IdtSetGate(17, (unsigned long)Isr17, IDT_TRAP_GATE);        // Alignment Check Exception
	IdtSetGate(18, (unsigned long)Isr18, IDT_TRAP_GATE);        // Machine Check Exception
	IdtSetGate(19, (unsigned long)Isr19, IDT_TRAP_GATE);        // SIMD Floating Point Exception
	IdtSetGate(20, (unsigned long)Isr20, IDT_TRAP_GATE);        // Virtualization Exception
	IdtSetGate(21, (unsigned long)Isr21, IDT_TRAP_GATE);        // Control Protection Exception
	IdtSetGate(22, (unsigned long)Isr22, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(23, (unsigned long)Isr23, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(24, (unsigned long)Isr24, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(25, (unsigned long)Isr25, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(26, (unsigned long)Isr26, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(27, (unsigned long)Isr27, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(28, (unsigned long)Isr28, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(29, (unsigned long)Isr29, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(30, (unsigned long)Isr30, IDT_TRAP_GATE);        // Reserved!
	IdtSetGate(31, (unsigned long)Isr31, IDT_TRAP_GATE);        // Reserved!

    // Loads the IDT table into the processor register (Assembler function)
    IdtFlush((unsigned long)&idtPointer);
}

// Installs the corresponding ISR routine in the IDT table
void IdtSetGate(unsigned char Entry, unsigned long BaseAddress, unsigned char Type)
{
    idtEntries[Entry].OffsetLow = (unsigned short)BaseAddress & 0xFFFF;
    idtEntries[Entry].Selector = 0x8;
    idtEntries[Entry].InterruptStackTable = 0;
    idtEntries[Entry].Reserved1 = 0;
    idtEntries[Entry].Type = Type;
    idtEntries[Entry].Reserved2 = 0;
    idtEntries[Entry].DPL = 0;
    idtEntries[Entry].Present = 1;
    idtEntries[Entry].OffsetMiddle = (unsigned short)((BaseAddress >> 16) & 0xFFFF);
    idtEntries[Entry].OffsetHigh = (unsigned int)((BaseAddress >> 32) & 0xFFFFFFFF);
    idtEntries[Entry].Reserved3 = 0;
}

// Our generic ISR handler
void IsrHandler(int Number, unsigned long cr2, RegisterState *Registers)
{
    // Display the occured exception
    DisplayException(Number, Registers);

    // Halt the system
    while (1 == 1) {}
}

// Displays the state of the general purpose registers when the exception has occured.
void DisplayException(int Number, RegisterState *Registers)
{
    printf("A fatal error has occured!\n");
    printf("ISR: 0x");
    printf_int(Number, 16);
    printf("\n");

    // Error Code
    printf("Error Code: ");
    printf_int(Registers->ErrorCode, 10);
    printf("\n");

    // RIP register
    printf("RIP: 0x");
    printf_long(Registers->RIP, 16);
    printf("\n");

    // RDI register
    printf("RDI: 0x");
    printf_long(Registers->RDI, 16);
    printf("\n");

    // RSI register
    printf("RSI: 0x");
    printf_long(Registers->RSI, 16);
    printf("\n");

    // RBP register
    printf("RBP: 0x");
    printf_long(Registers->RBP, 16);
    printf("\n");

    // RSP register
    printf("RSP: 0x");
    printf_long(Registers->RSP, 16);
    printf("\n");

    // RAX register
    printf("RAX: 0x");
    printf_long(Registers->RAX, 16);
    printf("\n");

    // RBX register
    printf("RBX: 0x");
    printf_long(Registers->RBX, 16);
    printf("\n");

    // RCX register
    printf("RCX: 0x");
    printf_long(Registers->RCX, 16);
    printf("\n");

    // RDX register
    printf("RDX: 0x");
    printf_long(Registers->RDX, 16);
    printf("\n");

    // R8 register
    printf("R8:  0x");
    printf_long(Registers->R8, 16);
    printf("\n");

    // R9 register
    printf("R9:  0x");
    printf_long(Registers->R9, 16);
    printf("\n");

    // R10 register
    printf("R10: 0x");
    printf_long(Registers->R10, 16);
    printf("\n");

    // R11 register
    printf("R11: 0x");
    printf_long(Registers->R11, 16);
    printf("\n");

    // R12 register
    printf("R12: 0x");
    printf_long(Registers->R12, 16);
    printf("\n");

    // R13 register
    printf("R13: 0x");
    printf_long(Registers->R13, 16);
    printf("\n");

    // R14 register
    printf("R14: 0x");
    printf_long(Registers->R14, 16);
    printf("\n");

    // R15 register
    printf("R15: 0x");
    printf_long(Registers->R15, 16);
    printf("\n");
}