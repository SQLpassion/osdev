#include "../../libc/syscall.h"
#include "../../libc/libc.h"
#include "shell.h"

// The available Shell commands
char *commands[] =
{
    "cls",
    "dir"
};

int (*command_functions[]) (char *param) =
{
    &shell_cls,
    &shell_dir
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

int shell_cls(char *param)
{
    ClearScreen();

    return 1;
}