#ifndef TASK_H
#define TASK_H

// The various Task states
#define TASK_STATUS_CREATED       0x0
#define TASK_STATUS_RUNNABLE      0x1
#define TASK_STATUS_RUNNING       0x2
#define TASK_STATUS_WAITING       0x3

// Represents a Task
typedef struct Task
{
    // General Purpose Registers
    unsigned long rax;
    unsigned long rbx;
    unsigned long rcx;
    unsigned long rdx;
    unsigned long rbp;
    unsigned long rsi;
    unsigned long r8;
    unsigned long r9;
    unsigned long r10;
    unsigned long r11;
    unsigned long r12;
    unsigned long r13;
    unsigned long r14;
    unsigned long r15;
    unsigned long cr3;

    unsigned long rdi;
    unsigned long rip;
    unsigned long cs;
    unsigned long rflags;
    unsigned long rsp;
    unsigned long ss;

    unsigned long ds;
    unsigned long es;
    unsigned long fs;
    unsigned long gs;

    // The ID of the running Task
    int PID;

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

// Creates a new Kernel Task
Task* CreateKernelModeTask(void *TaskCode, int PID, unsigned long KernelModeStack);

// Creates all initial OS tasks
void CreateInitialTasks();

// Moves the current Task from the head of the TaskList to the tail of the TaskList
Task* MoveToNextTask();

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