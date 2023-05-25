#include "../../libc/syscall.h"
#include "../../libc/libc.h"
#include "shell.h"

// The available Shell commands
char *commands[] =
{
    "cls",
    "ver"
};

int (*command_functions[]) (char *param) =
{
    &shell_cls,
    &shell_ver
};

// The main entry point for the User Mode program
void ShellMain()
{
    int i;
    
    while (1 == 1)
    {
        char input[100] = "";
        int commandFound = 0;
        printf("C:\>");
        scanf(input, 98);

        for (i = 0; i < COMMAND_COUNT; i++)
        {
            // Execute the specified command
            if (StartsWith(input, commands[i]) == 1)
            {
                (*command_functions[i])(&input);
                commandFound = 1;
            }
        }

        if (commandFound == 0)
        {
            ExecuteUserModeProgram("PROG1   BIN");
            /* // Try to load the requested program into memory
            if (LoadProgram(input) != 0)
            {
                // The program was loaded successfully into memory.
                // Let's execute it as a User Task!
                CreateUserTask(0xFFFF8000FFFF0000, 9, 0xFFFF800001900000, 0xFFFF800000090000);
            }
            else
            {
                printf("'");
                printf(input);
                printf("' is not recognized as an internal or external command,\n");
                printf("operable program or batch file.\n\n");
            } */
        }
    }
}

void shell_cls(char *param)
{
    printf("cls\n");
}

void shell_ver(char *param)
{
    printf("ver\n");
}