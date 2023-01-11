#ifndef IDT_H
#define IDT_H

// Virtual address where the IDT table is stored
#define IDT_START_OFFSET    0xFFFF800000060000

// Number of IDT entries
#define IDT_ENTRIES         256

// Constant for an Interrupt Gate
#define IDT_INTERRUPT_GATE  0xE

// Constant for a Trap Gate
#define IDT_TRAP_GATE       0xF

// Represents an Interrupt Gate - 128 Bit long
// As described in Volume 3A: 6.14.1
struct _idtEntry
{
    unsigned short OffsetLow;           // 16 Bit
    unsigned short Selector;            // 16 Bit
    unsigned InterruptStackTable : 3;   // 3 Bit
    unsigned Reserved1 : 5;             // 5 Bit
    unsigned Type : 4;                  // 4 Bit
    unsigned Reserved2 : 1;             // 1 Bit
    unsigned DPL : 2;                   // 2 Bit
    unsigned Present : 1;               // 1 Bit
    unsigned short OffsetMiddle;        // 16 Bit
    unsigned int OffsetHigh;            // 32 Bit
    unsigned int Reserved3;             // 32 Bit
} __attribute__ ((packed));
typedef struct _idtEntry IdtEntry;

// Represents the state of the registers when an exception has occured.
typedef struct _registerState
{
    unsigned long RIP;
    unsigned long ErrorCode;
    unsigned long RDI;
    unsigned long RSI;
    unsigned long RBP;
    unsigned long RSP;
    unsigned long RAX;
    unsigned long RBX;
    unsigned long RCX;
    unsigned long RDX;
    unsigned long R8;
    unsigned long R9;
    unsigned long R10;
    unsigned long R11;
    unsigned long R12;
    unsigned long R13;
    unsigned long R14;
    unsigned long R15;
} RegisterState;

// Represents the pointer to the interrupt gates
struct _idtPointer
{
    unsigned short Limit;
    unsigned long Base;
} __attribute((packed));
typedef struct _idtPointer IdtPointer;

// Initializes the IDT table for the ISR routines.
void InitIdt();

// Installs the corresponding ISR routine in the IDT table
void IdtSetGate(unsigned char Entry, unsigned long BaseAddress, unsigned char Type);

// Our generic ISR handler, which is called from the assembly code.
void IsrHandler(int InterruptNumber, unsigned long cr2, RegisterState *Registers);

// Displays the state of the general purpose registers when the exception has occured.
void DisplayException(int Number, RegisterState *Registers);

// Loads the IDT table into the processor register (implemented in Assembler)
extern void IdtFlush(unsigned long);

// The 32 ISR routines (implemented in Assembler)
extern void Isr0();     // Divide Error Exception
extern void Isr1();     // Debug Exception
extern void Isr2();     // Non-Maskable Interrupt
extern void Isr3();     // Breakpoint Exception
extern void Isr4();     // Overflow Exception
extern void Isr5();     // Bound Range Exceeded Exception
extern void Isr6();     // Invalid Opcode Exception
extern void Isr7();     // Device Not Available Exception
extern void Isr8();     // Double Fault Exception
extern void Isr9();     // Coprocessor Segment Overrun
extern void Isr10();    // Invalid TSS Exception
extern void Isr11();    // Segment Not Present
extern void Isr12();    // Stack Fault Exception
extern void Isr13();    // General Protection Exception
extern void Isr14();    // Page Fault Exception
extern void Isr15();    // Unassigned!
extern void Isr16();    // x87 FPU Floating Point Error
extern void Isr17();    // Alignment Check Exception
extern void Isr18();    // Machine Check Exception
extern void Isr19();    // SIMD Floating Point Exception
extern void Isr20();    // Virtualization Exception
extern void Isr21();    // Control Protection Exception
extern void Isr22();    // Reserved!
extern void Isr23();    // Reserved!
extern void Isr24();    // Reserved!
extern void Isr25();    // Reserved!
extern void Isr26();    // Reserved!
extern void Isr27();    // Reserved!
extern void Isr28();    // Reserved!
extern void Isr29();    // Reserved!
extern void Isr30();    // Reserved!
extern void Isr31();    // Reserved!

#endif