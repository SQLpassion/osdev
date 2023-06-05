#ifndef PROGRAM_H
#define PROGRAM_H

// The number of available commands
#define COMMAND_COUNT 3

// The main entry point for the User Mode program.
void ShellMain();

int shell_cls(char *param);
int shell_ver(char *param);
int shell_dir(char *param);

#endif