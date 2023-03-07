#include "task.h"
#include "../list.h"
#include "../memory/heap.h"
#include "../drivers/screen.h"

// Stores all Tasks to be executed
List *TaskList = 0x0;

// Creates a new Kernel Task
Task* CreateKernelModeTask(void *TaskCode, int PID, unsigned long KernelModeStack)
{
    Task *newTask = (Task *)malloc(sizeof(Task));
    newTask->KernelModeStack = KernelModeStack;
    newTask->rax = 0;
    newTask->rbx = 0;
    newTask->rcx = 0;
    newTask->rdx = 0;
    newTask->rbp = KernelModeStack;
    newTask->rsi = 0;
    newTask->r8 = 0;
    newTask->r9 = 0;
    newTask->r10 = 0;
    newTask->r11 = 0;
    newTask->r12 = 0;
    newTask->r13 = 0;
    // newTask->r14 = 0;           // Register R14 is currently not used, because it stores *globally* a reference to the KPCR Data Structure!
    newTask->r15 = (unsigned long)newTask;     // We store the state of the Task in register R15
    newTask->cr3 = 0x90000;     // Page Table Address
    newTask->rdi = 0;
    newTask->rip = (unsigned long)TaskCode;
    newTask->rflags = 0x2202;
    newTask->PID = PID;
    newTask->Status = TASK_STATUS_CREATED;

    // Set the Selectors for Ring 0
    newTask->cs = 0x8;
    newTask->ss = 0x10;
    newTask->ds = 0x10;

    // Prepare the stack of the new Task so that it looks like a traditional Stack Frame from an interrupt.
    // When we restore the state of this Task the first time, that Stack Frame is used during the IRETQ opcode.
    long *stack = KernelModeStack - 5;
    newTask->rsp = (unsigned long)stack;
    stack[0] = (unsigned long)TaskCode; // RIP
    stack[1] = 0x8;                     // Code Segment/Selector for Ring 0
    stack[2] = 0x2202;                  // RFLAGS
    stack[3] = KernelModeStack;         // Stack Pointer
    stack[4] = 0x10;                    // Stack Segment/Selector for Ring 0

    // Add the newly created Task to the end of the Task queue
    AddEntryToList(TaskList, newTask, PID);

    return newTask;
}

// Creates all initial OS tasks.
void CreateInitialTasks()
{
    // Initialize the TaskList
    TaskList = NewList();
    TaskList->PrintFunctionPtr = &PrintTaskList;

    // Create the Kernel Mode Tasks
    CreateKernelModeTask(Dummy1, 1, 0xFFFF800001100000);
    CreateKernelModeTask(Dummy2, 2, 0xFFFF800001200000);
    CreateKernelModeTask(Dummy3, 3, 0xFFFF800001300000);

    PrintList(TaskList);
    MoveToNextTask();
    PrintList(TaskList);
    MoveToNextTask();
    PrintList(TaskList);
    MoveToNextTask();
    PrintList(TaskList);
}

// Moves the current Task from the head of the TaskList to the tail of the TaskList.
Task* MoveToNextTask()
{
    // Remove the old head from the TaskList and set its status to TASK_STATUS_RUNNABLE
    ListEntry *oldHead = TaskList->RootEntry;
    ((Task *)oldHead->Payload)->Status = TASK_STATUS_RUNNABLE;
    RemoveEntryFromList(TaskList, oldHead, 0);

    // Add the old head to the end of the TaskList
    AddEntryToList(TaskList, oldHead->Payload, oldHead->Key);

    // Set the status of the new head to TASK_STATUS_RUNNING
    ((Task *)TaskList->RootEntry->Payload)->Status = TASK_STATUS_RUNNING;

    // Return the new head
    return ((Task *)TaskList->RootEntry->Payload);
}

// Prints out the TaskList entries
void PrintTaskList()
{
    ListEntry *currentEntry = TaskList->RootEntry;
    Task *task = (Task *)currentEntry->Payload;

    // Iterate over the whole list
    while (currentEntry != 0x0)
    {
        printf("PID: ");
        printf_long(task->PID, 10);
        printf(", KernelModeStack: 0x");
        printf_long(task->KernelModeStack, 16);
        printf(", Status: ");
        PrintStatus(task->Status);
        printf("\n");
    
        // Move to the next entry in the Double Linked List
        currentEntry = currentEntry->Next;
        task = (Task *)currentEntry->Payload;
    } 
}

// Prints out the status as text
static void PrintStatus(int Status)
{
    switch (Status)
    {
        case 0: printf("CREATED"); break;
        case 1: printf("RUNNABLE"); break;
        case 2: printf("RUNNING"); break;
        case 3:  printf("WAITING"); break;
    }
}

void Dummy1()
{
}

void Dummy2()
{
}

void Dummy3()
{
}