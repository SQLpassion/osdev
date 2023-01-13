#ifndef IRQ_H
#define IRQ_H

// Number of IDT entries
#define IRQ_ENTRIES 256

// Callback function pointer for handling the various IRQs
typedef void (*IRQ_HANDLER)(int Number);

// Registers a IRQ callback function
void RegisterIrqHandler(int n, IRQ_HANDLER Handler);

// IRQ handler that is called as soon as an IRQ is raised
void IrqHandler(int InterruptNumber);

// Our 15 IRQ routines (implemented in assembly code)
extern void Irq0();     // Timer
extern void Irq1();     // Keyboard
extern void Irq2();     // Cascade for 8259A Slave Controller
extern void Irq3();     // Serial Port 2
extern void Irq4();     // Serial Port 1
extern void Irq5();     // AT systems: Parallel Port 2. PS/2 systems: Reserved
extern void Irq6();     // Floppy Drive
extern void Irq7();     // Parallel Port 1
extern void Irq8();     // CMOS Real Time Clock
extern void Irq9();     // CGA Vertical Retrace
extern void Irq10();    // Reserved
extern void Irq11();    // Reserved
extern void Irq12();    // AT systems: Reserved. PS/2: Auxiliary Device
extern void Irq13();    // FPU
extern void Irq14();    // Hard Disk Controller
extern void Irq15();    // Reserved

#endif