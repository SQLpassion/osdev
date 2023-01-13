#ifndef KERNEL_H
#define KERNEL_H

// The main entry of our Kernel
void KernelMain();

// Initializes the whole Kernel
void InitKernel();

// Displays the BIOS Information Block
void DisplayBiosInformationBlock();

// Causes a Divide by Zero Exception
void DivideByZeroException();

// Tests the functionality of the keyboard
void KeyboardTest();

#endif