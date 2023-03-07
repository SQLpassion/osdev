#ifndef KERNEL_H
#define KERNEL_H

// The main entry of our Kernel
void KernelMain(int KernelSize);

// Initializes the whole Kernel
void InitKernel(int KernelSize);

// Causes a Divide by Zero Exception
void DivideByZeroException();

// Tests the functionality of the keyboard
void KeyboardTest();

#endif