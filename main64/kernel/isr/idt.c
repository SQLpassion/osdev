#include "idt.h"
#include "irq.h"
#include "../common.h"
#include "../multitasking/multitasking.h"
#include "../syscalls/syscall.h"
#include "../drivers/screen.h"
#include "../memory/virtual-memory.h"

// The 256 possible Interrupt Gates are stored from 0xFFFF800000060000 to 0xFFFF800000060FFF (4096 Bytes long - each Entry is 16 Bytes)
IdtEntry *idtEntries = (IdtEntry *)IDT_START_OFFSET;

// The pointer that points to the Interrupt Gates
IdtPointer idtPointer;

// Initializes the IDT table
void InitIdt()
{
    idtPointer.Limit = sizeof(IdtEntry) * IDT_ENTRIES - 1;
    idtPointer.Base = (unsigned long)idtEntries;
    memset(idtEntries, 0, sizeof(IdtEntry) * IDT_ENTRIES);

    // Setup the 32 Exception handlers - as described in Volume 3A: 6.15
    IdtSetGate(EXCEPTION_DIVIDE, (unsigned long)Isr0, IDT_TRAP_GATE);                       // Divide Error Exception
    IdtSetGate(EXCEPTION_DEBUG, (unsigned long)Isr1, IDT_TRAP_GATE);                        // Debug Exception
    IdtSetGate(EXCEPTION_NON_MASKABLE_INTERRUPT, (unsigned long)Isr2, IDT_TRAP_GATE);       // Non-Maskable Interrupt
    IdtSetGate(EXCEPTION_BREAKPOINT, (unsigned long)Isr3, IDT_TRAP_GATE);                   // Breakpoint Exception
    IdtSetGate(EXCEPTION_OVERFLOW, (unsigned long)Isr4, IDT_TRAP_GATE);                     // Overflow Exception
    IdtSetGate(EXCEPTION_BOUND_RANGE, (unsigned long)Isr5, IDT_TRAP_GATE);                  // Bound Range Exceeded Exception
    IdtSetGate(EXCEPTION_INVALID_OPCODE, (unsigned long)Isr6, IDT_TRAP_GATE);               // Invalid Opcode Exception
    IdtSetGate(EXCEPTION_DEVICE_NOT_AVAILABLE, (unsigned long)Isr7, IDT_TRAP_GATE);         // Device Not Available Exception
    IdtSetGate(EXCEPTION_DOUBLE_FAULT, (unsigned long)Isr8, IDT_INTERRUPT_GATE);            // Double Fault Exception
    IdtSetGate(EXCEPTION_COPROCESSOR_SEGMENT_OVERRUN, (unsigned long)Isr9, IDT_TRAP_GATE);  // Coprocessor Segment Overrun
    IdtSetGate(EXCEPTION_INVALID_TSS, (unsigned long)Isr10, IDT_INTERRUPT_GATE);            // Invalid TSS Exception
    IdtSetGate(EXCEPTION_SEGMENT_NOT_PRESENT, (unsigned long)Isr11, IDT_INTERRUPT_GATE);    // Segment Not Present
    IdtSetGate(EXCEPTION_STACK_FAULT, (unsigned long)Isr12, IDT_INTERRUPT_GATE);            // Stack Fault Exception
    IdtSetGate(EXCEPTION_GENERAL_PROTECTION, (unsigned long)Isr13, IDT_INTERRUPT_GATE);     // General Protection Exception
    IdtSetGate(EXCEPTION_PAGE_FAULT, (unsigned long)Isr14, IDT_INTERRUPT_GATE);             // Page Fault Exception
    IdtSetGate(EXCEPTION_UNASSGIGNED, (unsigned long)Isr15, IDT_TRAP_GATE);                 // Unassigned
    IdtSetGate(EXCEPTION_X87_FPU, (unsigned long)Isr16, IDT_TRAP_GATE);                     // x87 FPU Floating Point Error
    IdtSetGate(EXCEPTION_ALIGNMENT_CHECK, (unsigned long)Isr17, IDT_TRAP_GATE);             // Alignment Check Exception
    IdtSetGate(EXCEPTION_MACHINE_CHECK, (unsigned long)Isr18, IDT_TRAP_GATE);               // Machine Check Exception
    IdtSetGate(EXCEPTION_SIMD_FLOATING_POINT, (unsigned long)Isr19, IDT_TRAP_GATE);         // SIMD Floating Point Exception
    IdtSetGate(EXCEPTION_VIRTUALIZATION, (unsigned long)Isr20, IDT_TRAP_GATE);              // Virtualization Exception
    IdtSetGate(EXCEPTION_CONTROL_PROTECTION, (unsigned long)Isr21, IDT_TRAP_GATE);          // Control Protection Exception
    IdtSetGate(EXCEPTION_RESERVED_22, (unsigned long)Isr22, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_23, (unsigned long)Isr23, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_24, (unsigned long)Isr24, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_25, (unsigned long)Isr25, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_26, (unsigned long)Isr26, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_27, (unsigned long)Isr27, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_28, (unsigned long)Isr28, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_29, (unsigned long)Isr29, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_30, (unsigned long)Isr30, IDT_TRAP_GATE);                 // Reserved
    IdtSetGate(EXCEPTION_RESERVED_31, (unsigned long)Isr31, IDT_TRAP_GATE);                 // Reserved

    // Setup the 16 IRQ handlers
    IdtSetGate(32, (unsigned long)Irq0,  IDT_INTERRUPT_GATE);   // Timer
    IdtSetGate(33, (unsigned long)Irq1,  IDT_INTERRUPT_GATE);   // Keyboard
    IdtSetGate(34, (unsigned long)Irq2,  IDT_INTERRUPT_GATE);   // Cascade for 8259A Slave Controller
    IdtSetGate(35, (unsigned long)Irq3,  IDT_INTERRUPT_GATE);   // Serial Port 2
    IdtSetGate(36, (unsigned long)Irq4,  IDT_INTERRUPT_GATE);   // Serial Port 1
    IdtSetGate(37, (unsigned long)Irq5,  IDT_INTERRUPT_GATE);   // AT systems: Parallel Port 2. PS/2 systems: Reserved
    IdtSetGate(38, (unsigned long)Irq6,  IDT_INTERRUPT_GATE);   // Floppy Drive
    IdtSetGate(39, (unsigned long)Irq7,  IDT_INTERRUPT_GATE);   // Parallel Port 1
    IdtSetGate(40, (unsigned long)Irq8,  IDT_INTERRUPT_GATE);   // CMOS Real Time Clock
    IdtSetGate(41, (unsigned long)Irq9,  IDT_INTERRUPT_GATE);   // CGA Vertical Retrace
    IdtSetGate(42, (unsigned long)Irq10, IDT_INTERRUPT_GATE);   // Reserved
    IdtSetGate(43, (unsigned long)Irq11, IDT_INTERRUPT_GATE);   // Reserved
    IdtSetGate(44, (unsigned long)Irq12, IDT_INTERRUPT_GATE);   // AT systems: Reserved. PS/2: Auxiliary Device
    IdtSetGate(45, (unsigned long)Irq13, IDT_INTERRUPT_GATE);   // FPU
    IdtSetGate(46, (unsigned long)Irq14, IDT_INTERRUPT_GATE);   // Hard Disk Controller
    IdtSetGate(47, (unsigned long)Irq15, IDT_INTERRUPT_GATE);   // Reserved

    // The INT 0x80 can be raised from Ring 3
    IdtSetGate(128, (unsigned long)SysCallHandlerAsm, IDT_INTERRUPT_GATE);
    idtEntries[128].DPL = 3;

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

// Our generic ISR handler, which is called from the assembly code.
void IsrHandler(int InterruptNumber, unsigned long cr2, RegisterState *Registers)
{
    if (InterruptNumber == EXCEPTION_PAGE_FAULT)
    {
        // Handle the Page Fault
        HandlePageFault(cr2);
    }
    else
    {
        // Every other exception just stops the system
        
        // Display the occured exception
        DisplayException(InterruptNumber, Registers);

        // Halt the system
        while (1 == 1) {}
    }
}

// Installs the IRQ0 interrupt handler that performs the Context Switching between the various tasks
void InitTimerForContextSwitching()
{
    IdtSetGate(32, (unsigned long)Irq0_ContextSwitching, IDT_INTERRUPT_GATE);

    // Loads the IDT table into the processor register (Assembler function)
    IdtFlush((unsigned long)&idtPointer);
}

// Displays the state of the general purpose registers when the exception has occured.
void DisplayException(int Number, RegisterState *Registers)
{
    // Set the Blue Screen color
    unsigned int color = (COLOR_BLUE << 4) | (COLOR_WHITE & 0x0F);
    SetColor(color);
    ClearScreen();

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

    // SS register
    printf("SS: 0x");
    printf_long(Registers->SS, 16);

    // CS register
    printf(", CS: 0x");
    printf_long(Registers->CS, 16);

    // DS register
    printf(", DS: 0x");
    printf_long(Registers->DS, 16);

    // ES register
    printf(", ES: 0x");
    printf_long(Registers->ES, 16);

    // FS register
    printf(", FS: 0x");
    printf_long(Registers->FS, 16);

    // GS register
    printf(", GS: 0x");
    printf_long(Registers->GS, 16);
    printf("\n");

    // CR3 register
    printf("CR3: 0x");
    printf_long(Registers->CR3, 16);
    printf("\n");
}