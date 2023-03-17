#include "multitasking.h"
#include "gdt.h"
#include "../common.h"
#include "../list.h"
#include "../date.h"
#include "../memory/heap.h"
#include "../memory/virtual-memory.h"
#include "../drivers/screen.h"
#include "../syscalls/syscall.h"
#include "../io/fat12.h"

// Stores all Tasks to be executed
List *TaskList = 0x0;

// This counter stores how often Context Switching was performed, 
// and drives the system date calculation.
unsigned long counter = 0;

// Creates a new Kernel Mode Task
Task* CreateKernelModeTask(void *TaskCode, unsigned long PID, unsigned long KernelModeStack)
{
    // Allocate a new Task structure on the Heap
    Task *newTask = (Task *)malloc(sizeof(Task));
    newTask->KernelModeStack = KernelModeStack;
    newTask->PID = PID;
    newTask->Status = TASK_STATUS_CREATED;
    newTask->RIP = (unsigned long)TaskCode;
    newTask->CR3 = GetPML4Address();

    // The "Interrupt Enable Flag" (Bit 9) must be set
    newTask->RFLAGS = 0x200;

    // Set the General Purpose Registers
    newTask->RAX = 0x0;
    newTask->RBX = 0x0;
    newTask->RCX = 0x0;
    newTask->RDX = 0x0;
    newTask->RBP = KernelModeStack;
    newTask->RSP = KernelModeStack;
    newTask->RSI = 0x0;
    newTask->RDI = 0x0;
    newTask->R8 =  0x0;
    newTask->R9 =  0x0;
    newTask->R10 = 0x0;
    newTask->R11 = 0x0;
    newTask->R12 = 0x0;
    newTask->R13 = 0x0;
    newTask->R14 = 0x0;
    newTask->R15 = (unsigned long)newTask; // The address of the Task structure is stored in the R15 register

    // Set the Selectors for Ring 0
    newTask->CS = GDT_KERNEL_CODE_SEGMENT | RPL_RING0;
    newTask->DS = GDT_KERNEL_DATA_SEGMENT | RPL_RING0;
    newTask->SS = GDT_KERNEL_DATA_SEGMENT | RPL_RING0;

    // Set the remaining Segment Registers to zero
    newTask->ES = 0x0;
    newTask->FS = 0x0;
    newTask->GS = 0x0;

    // Touch the virtual address of the Kernel Mode Stack (8 bytes below the starting address), so that we can
    // be sure that the virtual address will get mapped to a physical Page Frame through the Page Fault Handler.
    // 
    // NOTE: If we don't do that, and the virtual address is unmapped, the OS will crash during the Context
    // Switching routine, because a Page Fault would occur (when we prepare the return Stack Frame), which
    // can'be be handled, because the interrupts are disabled!
    unsigned long *kernelModeStackPtr = (unsigned long *)KernelModeStack - 8;
    kernelModeStackPtr[0] = kernelModeStackPtr[0]; // This read/write operation causes a Page Fault!

    // Add the newly created Kernel Mode Task to the end of the TaskList
    AddEntryToList(TaskList, newTask, PID);

    // Return a reference to the newly created Kernel Mode Task
    return newTask;
}

// Creates a new User Mode Task
Task* CreateUserModeTask(void *TaskCode, unsigned long PID, unsigned long KernelModeStack, unsigned long UserModeStack)
{
    // Allocate a new Task structure on the Heap
    Task *newTask = (Task *)malloc(sizeof(Task));
    newTask->KernelModeStack = KernelModeStack;
    newTask->UserModeStack = UserModeStack;
    newTask->PID = PID;
    newTask->Status = TASK_STATUS_CREATED;
    newTask->RIP = (unsigned long)TaskCode;
    newTask->CR3 = GetPML4Address();

    // The "Interrupt Enable Flag" (Bit 9) must be set
    newTask->RFLAGS = 0x200;

    // Set the General Purpose Registers
    newTask->RAX = 0x0;
    newTask->RBX = 0x0;
    newTask->RCX = 0x0;
    newTask->RDX = 0x0;
    newTask->RBP = UserModeStack;
    newTask->RSP = UserModeStack;
    newTask->RSI = 0x0;
    newTask->RDI = 0x0;
    newTask->R8 =  0x0;
    newTask->R9 =  0x0;
    newTask->R10 = 0x0;
    newTask->R11 = 0x0;
    newTask->R12 = 0x0;
    newTask->R13 = 0x0;
    newTask->R14 = 0x0;
    newTask->R15 = (unsigned long)newTask; // The address of the Task structure is stored in the R15 register

    // Set the Selectors for Ring 3
    newTask->CS = GDT_USER_CODE_SEGMENT | RPL_RING3;
    newTask->DS = GDT_USER_DATA_SEGMENT | RPL_RING3;
    newTask->SS = GDT_USER_DATA_SEGMENT | RPL_RING3;

    // Set the remaining Segment Registers to zero
    newTask->ES = 0x0;
    newTask->FS = 0x0;
    newTask->GS = 0x0;

    // Touch the virtual address of the Kernel Mode Stack (8 bytes below the starting address), so that we can
    // be sure that the virtual address will get mapped to a physical Page Frame through the Page Fault Handler.
    // 
    // NOTE: If we don't do that, and the virtual address is unmapped, the OS will crash during the Context
    // Switching routine, because a Page Fault would occur (when we prepare the return Stack Frame), which
    // can'be be handled, because the interrupts are disabled!
    unsigned long *kernelModeStackPtr = (unsigned long *)KernelModeStack - 8;
    kernelModeStackPtr[0] = kernelModeStackPtr[0]; // This read/write operation causes a Page Fault!

    unsigned long *userModeStackPtr = (unsigned long *)UserModeStack - 8;
    userModeStackPtr[0] = userModeStackPtr[0]; // This read/write operation causes a Page Fault!
    
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

    if (LoadProgram("PROG1   BIN") != 0)
        CreateUserModeTask((void *)0xFFFF8000FFFF0000, 4, 0xFFFF800001400000, 0x00007FFFF0000000);

    // CreateKernelModeTask((void *)0xFFFF8000FFFF0000, 4, 0xFFFF800001400000);
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

    // Record the Context Switch
    ((Task *)TaskList->RootEntry->Payload)->ContextSwitches++;

    // Set the Kernel Mode Stack for the next executing Task
    TssEntry *tssEntry = GetTss();
    tssEntry->rsp0 = ((Task *)TaskList->RootEntry->Payload)->KernelModeStack;
    
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

// Terminates the Kernel Mode Task with the given PID
void TerminateTask(unsigned long PID)
{
    // Find the Task which needs to be terminated
    ListEntry *task = GetEntryFromList(TaskList, PID);

    // Remove the Task from the TaskList
    RemoveEntryFromList(TaskList, task);
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

// This function is executed in Kernel Mode - Ring 0
void Dummy1()
{
    while (1 == 1)
    {
        SetColor(COLOR_LIGHT_BLUE);
        
        // Print out the number of Context Switches
        Task *task = GetTaskState();
        printf_long(task->ContextSwitches, 10);
        printf("\n");
    }
}

// This function is executed in Kernel Mode - Ring 0
void Dummy2()
{
    while (1 == 1)
    {
        SetColor(COLOR_LIGHT_GREEN);

        // Print out the number of Context Switches
        Task *task = GetTaskState();
        printf_long(task->ContextSwitches, 10);
        printf("\n");
    }
}

// This function is executed in Kernel Mode - Ring 0
void Dummy3()
{
    while (1 == 1)
    {
        SetColor(COLOR_LIGHT_RED);
    
        // Print out the number of Context Switches
        Task *task = GetTaskState();
        printf_long(task->ContextSwitches, 10);
        printf("\n");
    }
}

// This function is executed in User Mode - Ring 3
void Dummy4()
{
    char buffer[32] = "";
    int a = 10;
    int b = 0;
    long counter = 0;
    // int c = a / b;

    while (1 == 1)
    {
        printf("Test...\n");

        /* // Calculate something...
        counter++;
        ltoa(counter, 10, buffer);
        
        // A direct printf() call doesn't work anymore in a User Mode task, 
        // because we can't access the Output Ports anymore, which are used
        // to move the screen cursor.
        // Therefore, we have to raise a SysCall into the Kernel Mode space...
        long result = SYSCALL3(SYSCALL_MUL, (void *)3, (void *)5, (void *)2);
        ltoa(result, 10, buffer);

        SYSCALL1(SYSCALL_PRINTF, "Hello World from USER Mode: ");
        SYSCALL1(SYSCALL_PRINTF, buffer);
        SYSCALL1(SYSCALL_PRINTF, "\n"); */
    }
}