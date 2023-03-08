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

// The various CPU exceptions
#define EXCEPTION_DIVIDE                        0
#define EXCEPTION_DEBUG                         1
#define EXCEPTION_NON_MASKABLE_INTERRUPT        2
#define EXCEPTION_BREAKPOINT                    3
#define EXCEPTION_OVERFLOW                      4
#define EXCEPTION_BOUND_RANGE                   5
#define EXCEPTION_INVALID_OPCODE                6
#define EXCEPTION_DEVICE_NOT_AVAILABLE          7
#define EXCEPTION_DOUBLE_FAULT                  8
#define EXCEPTION_COPROCESSOR_SEGMENT_OVERRUN   9
#define EXCEPTION_INVALID_TSS                   10
#define EXCEPTION_SEGMENT_NOT_PRESENT           11
#define EXCEPTION_STACK_FAULT                   12
#define EXCEPTION_GENERAL_PROTECTION            13
#define EXCEPTION_PAGE_FAULT                    14
#define EXCEPTION_UNASSGIGNED                   15
#define EXCEPTION_X87_FPU                       16
#define EXCEPTION_ALIGNMENT_CHECK               17
#define EXCEPTION_MACHINE_CHECK                 18
#define EXCEPTION_SIMD_FLOATING_POINT           19
#define EXCEPTION_VIRTUALIZATION                20
#define EXCEPTION_CONTROL_PROTECTION            21
#define EXCEPTION_RESERVED_22                   22
#define EXCEPTION_RESERVED_23                   23
#define EXCEPTION_RESERVED_24                   24
#define EXCEPTION_RESERVED_25                   25
#define EXCEPTION_RESERVED_26                   26
#define EXCEPTION_RESERVED_27                   27
#define EXCEPTION_RESERVED_28                   28
#define EXCEPTION_RESERVED_29                   29
#define EXCEPTION_RESERVED_30                   30
#define EXCEPTION_RESERVED_31                   31


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

// Installs the IRQ0 interrupt handler that performs the Context Switching between the various tasks
void InitTimerForContextSwitching();

// Loads the IDT table into the processor register (implemented in Assembler)
extern void IdtFlush(unsigned long);

// Disables the hardware interrupts
extern void DisableInterrupts();

// Enables the hardware interrupts
extern void EnableInterrupts();

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