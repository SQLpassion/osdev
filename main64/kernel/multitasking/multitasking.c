#include "multitasking.h"
#include "../common.h"
#include "../list.h"
#include "../date.h"
#include "../memory/heap.h"
#include "../memory/virtual-memory.h"
#include "../drivers/screen.h"

// Stores all Tasks to be executed
List *TaskList = 0x0;

// This counter stores how often Context Switching was performed, 
// and drives the system date calculation.
unsigned long counter = 0;

// Creates a new Kernel Mode Task
Task* CreateKernelModeTask(void *TaskCode, int PID, unsigned long KernelModeStack)
{
    Task *newTask = (Task *)malloc(sizeof(Task));
    newTask->KernelModeStack = KernelModeStack;
    newTask->PID = PID;
    newTask->Status = TASK_STATUS_CREATED;
    newTask->rip = (unsigned long)TaskCode;

    // 	The "Interrupt Enable Flag" (Bit 9) must be set
    newTask->rflags = 0x200;

    // Set the General Purpose Registers
    newTask->rax = 0x0;
    newTask->rbx = 0x0;
    newTask->rcx = 0x0;
    newTask->rdx = 0x0;
    newTask->rbp = KernelModeStack;
    newTask->rsp = KernelModeStack;
    newTask->rsi = 0x0;
    newTask->rdi = 0x0;
    newTask->r8 = 0x0;
    newTask->r9 = 0x0;
    newTask->r10 = 0x0;
    newTask->r11 = 0x0;
    newTask->r12 = 0x0;
    newTask->r13 = 0x0;
    newTask->r14 = 0x0;
    newTask->r15 = (unsigned long)newTask; // The address of the Task structure is stored in the R15 register

    // Set the remaining segment registers
    newTask->es = 0x0;
    newTask->fs = 0x0;
    newTask->gs = 0x0;

    // Set the Selectors for Ring 0
    newTask->cs = 0x8;
    newTask->ss = 0x10;
    newTask->ds = 0x10;

    // Touch the virtual address of the Kernel Mode Stack (8 bytes below the starting address), so that we can
    // be sure that the virtual address will get mapped to a physical Page Frame through the Page Fault Handler.
    // 
    // NOTE: If we don't do that, and the virtual address is unmapped, the OS will crash during the Context
    // Switching routine, because a Page Fault would occur (when we prepare the return Stack Frame), which
    // can'be be handled, because the interrupts are disabled!
    unsigned long *ptr = (unsigned long *)KernelModeStack - 8;
    ptr[0] = ptr[0]; // This read/write operation causes a Page Fault!

    // Add the newly created Kernel Mode Task to the end of the TaskList
    AddEntryToList(TaskList, newTask, PID);

    // Return a reference to the newly created Kernel Mode Task
    return newTask;
}

// Creates all initial OS tasks.
void CreateInitialTasks()
{
    // Initialize the TaskList
    TaskList = NewList();
    TaskList->PrintFunctionPtr = &PrintTaskList;

    // Create the initial Kernel Mode Tasks
    CreateKernelModeTask(Dummy1, 1, 0xFFFF800001100000);
    CreateKernelModeTask(Dummy2, 2, 0xFFFF800001200000);
    CreateKernelModeTask(Dummy3, 3, 0xFFFF800001300000);
}

// Moves the current Task from the head of the TaskList to the tail of the TaskList.
Task* MoveToNextTask()
{
    // Remove the old head from the TaskList and set its status to TASK_STATUS_RUNNABLE
    ListEntry *oldHead = TaskList->RootEntry;
    ((Task *)oldHead->Payload)->Status = TASK_STATUS_RUNNABLE;
    RemoveEntryFromList(TaskList, oldHead);

    // Add the old head to the end of the TaskList
    AddEntryToList(TaskList, oldHead->Payload, oldHead->Key);

    // Set the status of the new head to TASK_STATUS_RUNNING
    ((Task *)TaskList->RootEntry->Payload)->Status = TASK_STATUS_RUNNING;

    // Increment the clock counter
    counter++;

    // The timer is fired every 4 milliseconds
    if (counter % 250 == 0)
    {
        // Increment the system date by 1 second
        IncrementSystemDate();

        // Refresh the status line
        RefreshStatusLine();
    }

    // Return the new head
    return ((Task *)TaskList->RootEntry->Payload);
}

// Refreshs the status line
void RefreshStatusLine()
{
    char buffer[80] = "";
    char str[32] = "";
    char tmp[2] = "";

    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

    // Print out the year
    itoa(bib->Year, 10, str);
    strcat(buffer, str);
    strcat(buffer, "-");

    // Print out the month
    FormatInteger(bib->Month, tmp);
    strcat(buffer, tmp);
    strcat(buffer, "-");

    // Print out the day
    FormatInteger(bib->Day, tmp);
    strcat(buffer, tmp);
    strcat(buffer, ", ");

    // Print out the hour
    FormatInteger(bib->Hour, tmp);
    strcat(buffer, tmp);
    strcat(buffer, ":");

    // Print out the minute
    FormatInteger(bib->Minute, tmp);
    strcat(buffer, tmp);
    strcat(buffer, ":");

    // Print out the second
    FormatInteger(bib->Second, tmp);
    strcat(buffer, tmp);

    // Print out the available memory
    strcat(buffer, ", PMEM: ");
    ltoa(bib->MaxMemory / 1024 / 1024 + 1, 10, str);
    strcat(buffer, str);
    strcat(buffer, " MB, FMEM: ");
    ltoa(bib->AvailablePageFrames, 10, str);
    strcat(buffer, str);
    strcat(buffer, " Page Frames");

    // Pad the remaining columns with a blank, so that the status line goes
    // over the whole row
    int len = 80 - strlen(buffer);

    while (len > 0)
    {
        strcat(buffer, " ");
        len--;
    }

    // Print out the status line
    PrintStatusLine(buffer);
}

// Prints out the TaskList entries
void PrintTaskList()
{
    ListEntry *currentEntry = TaskList->RootEntry;
    Task *task = (Task *)currentEntry->Payload;

    // Iterate over the whole list
    while (currentEntry != 0x0)
    {
        printf("0x");
        printf_long((unsigned long)currentEntry, 16);
        printf(", PID: ");
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

    printf("\n");
}

// Prints out the status as text
static void PrintStatus(int Status)
{
    switch (Status)
    {
        case 0: printf("CREATED"); break;
        case 1: printf("RUNNABLE"); break;
        case 2: printf("RUNNING"); break;
        case 3: printf("WAITING"); break;
    }
}

void Dummy1()
{
    while (1 == 1)
    {
        SetColor(COLOR_LIGHT_BLUE);
        printf("1");
        printf("\n");

        // void *ptr = malloc(100);
    }
}

void Dummy2()
{
    while (1 == 1)
    {
        SetColor(COLOR_LIGHT_GREEN);
        printf("2");
        printf("\n");

        // void *ptr = malloc(100);
    }
}

void Dummy3()
{
    while (1 == 1)
    {
        SetColor(COLOR_LIGHT_RED);
        printf("3");
        printf("\n");

        // void *ptr = malloc(100);
    }
}