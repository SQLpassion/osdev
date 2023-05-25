#ifndef SYSCALL_H
#define SYSCALL_H

// Defines the various available SysCalls
#define SYSCALL_PRINTF              1
#define SYSCALL_GETPID              2
#define SYSCALL_TERMINATE_PROCESS   3
#define SYSCALL_GETCHAR             4
#define SYSCALL_GETCURSOR           5
#define SYSCALL_SETCURSOR           6
#define SYSCALL_EXECUTE             7

// Raises a SysCall with no parameters
long SYSCALL0(int SysCallNumber);
extern long SYSCALLASM0();

// Raises a SysCall with 1 parameter
long SYSCALL1(int SysCallNumber, void *Parameter1);
extern long SYSCALLASM1();

// Raises a Syscall with 2 parameters
long SYSCALL2(int SysCallNumber, void *Parameter1, void *Parameter2);
extern long SYSCALLASM2();

// Raises a Syscall with 3 parameters
long SYSCALL3(int SysCallNumber, void *Parameter1, void *Parameter2, void *Parameter3);
extern long SYSCALLASM3();

#endif