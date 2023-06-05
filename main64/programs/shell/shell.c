#include "../../libc/syscall.h"
#include "../../libc/libc.h"
#include "shell.h"

// The available Shell commands
char *commands[] =
{
    "cls",
    "ver",
    "dir"
};

int (*command_functions[]) (char *param) =
{
    &shell_cls,
    &shell_ver,
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

int shell_cls(char *param)
{
    printf("cls\n");

    return 0;
}

int shell_ver(char *param)
{
    printf("ver\n");

    return 0;
}

int shell_dir(char *param)
{
    printf("dir\n");

    return 0;
}