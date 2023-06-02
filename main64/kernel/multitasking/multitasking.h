#ifndef TASK_H
#define TASK_H

// The various Task states
#define TASK_STATUS_CREATED             0x0
#define TASK_STATUS_RUNNABLE            0x1
#define TASK_STATUS_RUNNING             0x2
#define TASK_STATUS_WAITING             0x3

#define EXECUTABLE_BASE_ADDRESS         0x0000700000000000
#define EXECUTABLE_USERMODE_STACK       0x00007FFFF0000000
#define EXECUTABLE_KERNELMODE_STACK     0xFFFF800001400000

#define USERMODE_PROGRAMM_TO_EXECUTE    0xFFFF800000300000

// Represents the state of a Task
typedef struct Task
{
    // Instruction Pointer and Flags Registers
    unsigned long RIP;      // Offset   +0
    unsigned long RFLAGS;   // Offset   +8
    
    // General Purpose Registers
    unsigned long RAX;      // Offset  +16
    unsigned long RBX;      // Offset  +24
    unsigned long RCX;      // Offset  +32
    unsigned long RDX;      // Offset  +40
    unsigned long RSI;      // Offset  +48
    unsigned long RDI;      // Offset  +56
    unsigned long RBP;      // Offset  +64
    unsigned long RSP;      // Offset  +72
    unsigned long R8;       // Offset  +80
    unsigned long R9;       // Offset  +88
    unsigned long R10;      // Offset  +96
    unsigned long R11;      // Offset +104
    unsigned long R12;      // Offset +112
    unsigned long R13;      // Offset +120
    unsigned long R14;      // Offset +128
    unsigned long R15;      // Offset +136
    
    // Segment Registers
    unsigned long SS;       // Offset +144
    unsigned long CS;       // Offset +152
    unsigned long DS;       // Offset +160
    unsigned long ES;       // Offset +168
    unsigned long FS;       // Offset +176
    unsigned long GS;       // Offset +184

    // Control Registers
    unsigned long CR3;      // Offset +192

    // The ID of the running Task
    unsigned long PID;

    // The used Kernel Mode Stack
    unsigned long KernelModeStack;

    // The used User Mode Stack
    unsigned long UserModeStack;

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

// Creates a new User Mode Task
Task* ExecuteUserModeProgram(unsigned char *FileName, unsigned long PID);

// Creates a new User Mode Task
Task* ExecuteUserModeProgramNew(unsigned char *FileName, unsigned long PID, unsigned long OriginalCR3);

// Loads the given program into a new User Mode Virtual Address Space
static int LoadProgramIntoUserModeVirtualAddressSpace(unsigned char *FileName, unsigned long UserModePML4Table);

// Loads the given program into a new User Mode Virtual Address Space
static int LoadProgramIntoUserModeVirtualAddressSpaceNew(unsigned char *FileName, unsigned long UserModePML4Table, unsigned long OriginalCR3);

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

// This function continuously checks if there is a new User Mode program to be started
void StartUserModeTask();

// Prints out the status as text
static void PrintStatus(int Status);

void Dummy1();
void Dummy2();
void Dummy3();
void Dummy4();

#endif