#ifndef PROGRAM_H
#define PROGRAM_H

// The number of available commands
#define COMMAND_COUNT 2

// The main entry point for the User Mode program.
void ShellMain();

void shell_cls(char *param);
void shell_ver(char *param);

#endif