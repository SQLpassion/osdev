#ifndef TASK_H
#define TASK_H

// The various Task states
#define TASK_STATUS_CREATED       0x0
#define TASK_STATUS_RUNNABLE      0x1
#define TASK_STATUS_RUNNING       0x2
#define TASK_STATUS_WAITING       0x3

// Represents the state of a Task
typedef struct Task
{
    // Instruction Pointer and Flags Registers
    unsigned long rip;      // Offset   +0
    unsigned long rflags;   // Offset   +8
    
    // General Purpose Registers
    unsigned long rax;      // Offset  +16
    unsigned long rbx;      // Offset  +24
    unsigned long rcx;      // Offset  +32
    unsigned long rdx;      // Offset  +40
    unsigned long rsi;      // Offset  +48
    unsigned long rdi;      // Offset  +56
    unsigned long rbp;      // Offset  +64
    unsigned long rsp;      // Offset  +72
    unsigned long r8;       // Offset  +80
    unsigned long r9;       // Offset  +88
    unsigned long r10;      // Offset  +96
    unsigned long r11;      // Offset +104
    unsigned long r12;      // Offset +112
    unsigned long r13;      // Offset +120
    unsigned long r14;      // Offset +128
    unsigned long r15;      // Offset +136
    
    // Segment Registers
    unsigned long ss;       // Offset +144
    unsigned long cs;       // Offset +152
    unsigned long ds;       // Offset +160
    unsigned long es;       // Offset +168
    unsigned long fs;       // Offset +176
    unsigned long gs;       // Offset +184

    // Control Registers
    unsigned long cr3;      // Offset +192

    // The ID of the running Task
    unsigned long PID;

    // The used Kernel Mode Stack
    unsigned long KernelModeStack;

    // The number of context switches of the running Task
    unsigned long ContextSwitches;

    // The status of the Task:
    // 0: CREATED
    // 1: RUNNABLE
    // 2: RUNNING
    // 3: WAITING
    int Status;
} Task;

// The Context Switching routine implemented in Assembler
extern void Irq0_ContextSwitching();

// The GetTaskState function implemented in Assembler
extern Task *GetTaskState();

// Creates a new Kernel Task
Task* CreateKernelModeTask(void *TaskCode, unsigned long PID, unsigned long KernelModeStack);

// Creates all initial OS tasks
void CreateInitialTasks();

// Moves the current Task from the head of the TaskList to the tail of the TaskList
Task* MoveToNextTask();

// Terminates the Kernel Mode Task with the given PID
void TerminateTask(unsigned long PID);

// Refreshs the status line
void RefreshStatusLine();

// Prints out the TaskList entries
void PrintTaskList();

// Prints out the status as text
static void PrintStatus(int Status);

void Dummy1();
void Dummy2();
void Dummy3();

#endif