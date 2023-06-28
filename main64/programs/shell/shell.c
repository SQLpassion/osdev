#include "../../libc/syscall.h"
#include "../../libc/libc.h"
#include "shell.h"

// The available Shell commands
char *commands[] =
{
    "cls",
    "dir",
    "mkfile",
    "type",
    "del",
    "open"
};

int (*command_functions[]) (char *param) =
{
    &shell_cls,
    &shell_dir,
    &shell_mkfile,
    &shell_type,
    &shell_del,
    &shell_open
};

// The main entry point for the User Mode program
void ShellMain()
{
    int i;
    
    while (1 == 1)
    {
        char input[100] = "";
        int commandFound = 0;
        printf("C:\\>");
        scanf(input, 98);

        for (i = 0; i < COMMAND_COUNT; i++)
        {
            // Execute the specified command
            if (StartsWith(input, commands[i]) == 1)
            {
                (*command_functions[i])((char *)&input);
                commandFound = 1;
            }
        }

        if (commandFound == 0)
        {
            // Execute the requested User Mode program...
            if (ExecuteUserModeProgram(input) == 0)
            {
                printf("'");
                printf(input);
                printf("' is not recognized as an internal or external command,\n");
                printf("operable program or batch file.\n\n");
            }
        }
    }
}

// Prints out the Root Directory of the FAT12 partition
int shell_dir(char *param)
{
    PrintRootDirectory();

    return 1;
}

// Clears the screen
int shell_cls(char *param)
{
    ClearScreen();

    return 1;
}

// Creates a new file
int shell_mkfile(char *param)
{
    char fileName[10] = "";
    char extension[5] = "";
    char content[512] = "";
    
    printf("Please enter the name of the new file: ");
    scanf(fileName, 8);
    printf("Please enter the extension of the new file: ");
    scanf(extension, 3);
    printf("Please enter the inital content of the new file: ");
    scanf(content, 510);

    CreateFile(fileName, extension, content);
    ClearScreen();
    printf("The file was created successfully.\n");
}

// Prints out an existing file
int shell_type(char *param)
{
    char fileName[10] = "";
    char extension[5] = "";

    printf("Please enter the name of the file to be printed out: ");
    scanf(fileName, 8);
    printf("Please enter the extension of the file to be printed out: ");
    scanf(extension, 3);
   
    unsigned char buffer[510] = "";
    unsigned long fileHandle = OpenFile(fileName, extension);

    while (!EndOfFile(fileHandle))
    {
        ReadFile(fileHandle, (unsigned char *)&buffer, 500);
        printf((unsigned char *)&buffer);
    }

    printf("\n");

    CloseFile(fileHandle);
}

// Deletes an existing file
int shell_del(char *param)
{
    char fileName[10] = "";
    char extension[5] = "";

    printf("Please enter the name of the file to be deleted: ");
    scanf(fileName, 8);
    printf("Please enter the extension of the file to be deleted: ");
    scanf(extension, 3);

    DeleteFile(fileName, extension);
    printf("The file was deleted successfully.\n");
}

int shell_open(char *param)
{
    unsigned long fileHandle1 = OpenFile("PROG1   ", "BIN");
    unsigned long fileHandle2 = OpenFile("PROG2   ", "BIN");
    unsigned long fileHandle3 = OpenFile("SHELL   ", "BIN");
    
    CloseFile(fileHandle2);
    CloseFile(fileHandle1);
    CloseFile(fileHandle3);
}